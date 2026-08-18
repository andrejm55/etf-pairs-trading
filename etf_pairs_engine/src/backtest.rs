use anyhow::{anyhow, Result};
use chrono::{DateTime, NaiveDate, Utc};
use std::collections::{BTreeSet, HashMap};
use std::fs;

use crate::alpaca::{AlpacaClient, Quote, Side};
use crate::analytics;
use crate::config::{AppConfig, PairConfig};
use crate::execution::balanced_quantities;
use crate::pairs::{DecisionAction, PairDecision, PairEngine};
use crate::risk::RiskEngine;

#[derive(Debug, Clone)]
pub struct BacktestOptions {
    pub from: String,
    pub to: String,
    pub timeframe: String,
    pub feed: String,
    pub limit: usize,
    pub slippage_bps: f64,
    pub trades_csv: Option<String>,
    pub report_dir: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BacktestCsvOptions {
    pub bars_csv: String,
    pub pair_id: String,
    pub a: String,
    pub b: String,
    pub beta: f64,
    pub rolling_window_ticks: usize,
    pub min_samples: usize,
    pub entry_zscore: f64,
    pub exit_zscore: f64,
    pub max_holding_bars: u64,
    pub use_rolling_beta: Option<bool>,
    pub beta_min: Option<f64>,
    pub beta_max: Option<f64>,
    pub min_correlation: Option<f64>,
    pub max_spread_std_bps: Option<f64>,
    pub entry_confirmation_bars: Option<u64>,
    pub min_expected_spread_move_after_costs: Option<f64>,
    pub adverse_zscore: Option<f64>,
    pub stop_loss_dollars: Option<f64>,
    pub profit_protection_min_dollars: Option<f64>,
    pub profit_protection_retrace_fraction: Option<f64>,
    pub profit_protection_floor_dollars: Option<f64>,
    pub leg_notional: f64,
    pub slippage_bps: f64,
    pub trades_csv: Option<String>,
    pub report_dir: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WalkForwardCsvOptions {
    pub bars_csv: String,
    pub pair_id: String,
    pub a: String,
    pub b: String,
    pub beta: f64,
    pub train_bars: usize,
    pub test_bars: usize,
    pub min_train_trades: usize,
    pub leg_notional: f64,
    pub slippage_bps: f64,
    pub output_csv: Option<String>,
    pub report_dir: Option<String>,
}

#[derive(Debug, Clone)]
struct SimLeg {
    symbol: String,
    qty: f64,
    side: Side,
    entry_price: f64,
}

#[derive(Debug, Clone)]
struct SimPosition {
    pair_id: String,
    opened_at: DateTime<Utc>,
    opened_index: usize,
    a: SimLeg,
    b: SimLeg,
    entry_z: f64,
    max_open_pnl: f64,
}

#[derive(Debug, Clone)]
struct SimTrade {
    pair_id: String,
    opened_at: DateTime<Utc>,
    closed_at: DateTime<Utc>,
    opened_index: usize,
    closed_index: usize,
    symbol_a: String,
    symbol_b: String,
    side_a: Side,
    side_b: Side,
    holding_bars: usize,
    qty_a: f64,
    qty_b: f64,
    entry_price_a: f64,
    entry_price_b: f64,
    exit_price_a: f64,
    exit_price_b: f64,
    entry_z: f64,
    exit_z: f64,
    pnl_a: f64,
    pnl_b: f64,
    pnl: f64,
    estimated_costs: f64,
    reason: String,
}

#[derive(Debug, Clone)]
struct SimReport {
    data_start: Option<DateTime<Utc>>,
    data_end: Option<DateTime<Utc>>,
    bars: usize,
    trades: Vec<SimTrade>,
    equity_curve: Vec<f64>,
}

impl SimReport {
    fn total_pnl(&self) -> f64 {
        self.trades.iter().map(|t| t.pnl).sum()
    }

    fn wins(&self) -> usize {
        self.trades.iter().filter(|t| t.pnl > 0.0).count()
    }

    fn win_rate(&self) -> f64 {
        if self.trades.is_empty() {
            0.0
        } else {
            self.wins() as f64 / self.trades.len() as f64 * 100.0
        }
    }

    fn avg_trade_pnl(&self) -> f64 {
        if self.trades.is_empty() {
            0.0
        } else {
            self.total_pnl() / self.trades.len() as f64
        }
    }

    fn max_drawdown(&self) -> f64 {
        max_drawdown(&self.equity_curve)
    }
}

#[derive(Debug, Clone, Copy)]
struct ParamSet {
    window: usize,
    min_samples: usize,
    entry_zscore: f64,
    exit_zscore: f64,
    max_holding_bars: u64,
    use_rolling_beta: Option<bool>,
    beta_min: Option<f64>,
    beta_max: Option<f64>,
    min_correlation: Option<f64>,
    max_spread_std_bps: Option<f64>,
    entry_confirmation_bars: Option<u64>,
    min_expected_spread_move_after_costs: Option<f64>,
    adverse_zscore: Option<f64>,
    stop_loss_dollars: Option<f64>,
    profit_protection_min_dollars: Option<f64>,
    profit_protection_retrace_fraction: Option<f64>,
    profit_protection_floor_dollars: Option<f64>,
}

struct WalkForwardRow {
    fold: usize,
    train_start: DateTime<Utc>,
    train_end: DateTime<Utc>,
    test_start: DateTime<Utc>,
    test_end: DateTime<Utc>,
    params: ParamSet,
    train: SimReport,
    test: SimReport,
}

pub async fn run(cfg: AppConfig, opts: BacktestOptions) -> Result<()> {
    let from = parse_start(&opts.from)?;
    let to = parse_end(&opts.to)?;
    let client = AlpacaClient::from_config(&cfg.alpaca)?;
    let bars = client
        .historical_bars_with_feed(
            &cfg.symbol_universe(),
            &opts.timeframe,
            from,
            to,
            &opts.feed,
            opts.limit,
        )
        .await?;
    run_loaded_bars(
        cfg,
        ReportContext {
            source: "alpaca".into(),
            timeframe: opts.timeframe,
            feed: Some(opts.feed),
            requested_from: Some(from),
            requested_to: Some(to),
        },
        opts.slippage_bps,
        opts.trades_csv,
        opts.report_dir,
        bars,
    )
    .await
}

pub async fn run_csv(mut cfg: AppConfig, opts: BacktestCsvOptions) -> Result<()> {
    cfg.strategy.rolling_window_ticks = opts.rolling_window_ticks;
    cfg.strategy.min_samples = opts.min_samples;
    cfg.strategy.entry_zscore = opts.entry_zscore;
    cfg.strategy.exit_zscore = opts.exit_zscore;
    cfg.risk.max_holding_minutes = opts.max_holding_bars;
    cfg = configured_single_pair(
        cfg,
        &opts.pair_id,
        &opts.a,
        &opts.b,
        opts.beta,
        opts.leg_notional,
        Some(ParamSet {
            window: opts.rolling_window_ticks,
            min_samples: opts.min_samples,
            entry_zscore: opts.entry_zscore,
            exit_zscore: opts.exit_zscore,
            max_holding_bars: opts.max_holding_bars,
            use_rolling_beta: opts.use_rolling_beta,
            beta_min: opts.beta_min,
            beta_max: opts.beta_max,
            min_correlation: opts.min_correlation,
            max_spread_std_bps: opts.max_spread_std_bps,
            entry_confirmation_bars: opts.entry_confirmation_bars,
            min_expected_spread_move_after_costs: opts.min_expected_spread_move_after_costs,
            adverse_zscore: opts.adverse_zscore,
            stop_loss_dollars: opts.stop_loss_dollars,
            profit_protection_min_dollars: opts.profit_protection_min_dollars,
            profit_protection_retrace_fraction: opts.profit_protection_retrace_fraction,
            profit_protection_floor_dollars: opts.profit_protection_floor_dollars,
        }),
    );

    let bars = read_bars_csv(&opts.bars_csv, &cfg.symbol_universe())?;
    run_loaded_bars(
        cfg,
        ReportContext {
            source: opts.bars_csv,
            timeframe: "csv".into(),
            feed: None,
            requested_from: None,
            requested_to: None,
        },
        opts.slippage_bps,
        opts.trades_csv,
        opts.report_dir,
        bars,
    )
    .await
}

pub async fn run_walk_forward_csv(cfg: AppConfig, opts: WalkForwardCsvOptions) -> Result<()> {
    let cfg = configured_single_pair(
        cfg,
        &opts.pair_id,
        &opts.a,
        &opts.b,
        opts.beta,
        opts.leg_notional,
        None,
    );
    let bars = read_bars_csv(&opts.bars_csv, &cfg.symbol_universe())?;
    let timeline = aligned_timeline(&bars);
    if timeline.len() < opts.train_bars + opts.test_bars {
        return Err(anyhow!(
            "not enough aligned bars for walk-forward: have {}, need at least {}",
            timeline.len(),
            opts.train_bars + opts.test_bars
        ));
    }

    let grid = default_walk_forward_grid();
    let mut rows = Vec::new();
    let mut test_trades = 0usize;
    let mut test_wins = 0usize;
    let mut test_pnl = 0.0;
    let mut test_dd_sum = 0.0;
    let mut fold = 1usize;
    let mut train_start_idx = 0usize;

    while train_start_idx + opts.train_bars + opts.test_bars <= timeline.len() {
        let train_end_idx = train_start_idx + opts.train_bars - 1;
        let test_start_idx = train_end_idx + 1;
        let test_end_idx = test_start_idx + opts.test_bars - 1;
        let train_bars = slice_bars(&bars, timeline[train_start_idx], timeline[train_end_idx]);
        let test_bars = slice_bars(&bars, timeline[test_start_idx], timeline[test_end_idx]);

        let mut best: Option<(ParamSet, SimReport, f64)> = None;
        for params in &grid {
            let candidate_cfg = configured_single_pair(
                cfg.clone(),
                &opts.pair_id,
                &opts.a,
                &opts.b,
                opts.beta,
                opts.leg_notional,
                Some(*params),
            );
            let train =
                simulate_loaded_bars(candidate_cfg, opts.slippage_bps, train_bars.clone()).await?;
            let score = walk_forward_score(&train, opts.min_train_trades);
            if best
                .as_ref()
                .map(|(_, _, best_score)| score > *best_score)
                .unwrap_or(true)
            {
                best = Some((*params, train, score));
            }
        }

        let Some((params, train, _score)) = best else {
            return Err(anyhow!("walk-forward grid was empty"));
        };
        let test_cfg = configured_single_pair(
            cfg.clone(),
            &opts.pair_id,
            &opts.a,
            &opts.b,
            opts.beta,
            opts.leg_notional,
            Some(params),
        );
        let test = simulate_loaded_bars(test_cfg, opts.slippage_bps, test_bars).await?;

        test_trades += test.trades.len();
        test_wins += test.wins();
        test_pnl += test.total_pnl();
        test_dd_sum += test.max_drawdown();
        rows.push(WalkForwardRow {
            fold,
            train_start: timeline[train_start_idx],
            train_end: timeline[train_end_idx],
            test_start: timeline[test_start_idx],
            test_end: timeline[test_end_idx],
            params,
            train,
            test,
        });

        fold += 1;
        train_start_idx += opts.test_bars;
    }

    if let Some(path) = &opts.output_csv {
        write_walk_forward_csv(path, &rows)?;
    }
    if let Some(report_dir) = &opts.report_dir {
        write_walk_forward_report_dir(report_dir, &opts, &timeline, &rows)?;
    }

    println!("Walk-forward report");
    println!("source: {}", opts.bars_csv);
    println!("pair: {}", opts.pair_id);
    println!(
        "data_range: {} to {}",
        timeline[0].date_naive(),
        timeline[timeline.len() - 1].date_naive()
    );
    println!("train_bars: {}", opts.train_bars);
    println!("test_bars: {}", opts.test_bars);
    println!("folds: {}", rows.len());
    println!("slippage_bps per fill: {:.2}", opts.slippage_bps);
    println!("test_trades: {test_trades}");
    println!("test_wins: {test_wins}");
    let test_win_rate = if test_trades == 0 {
        0.0
    } else {
        test_wins as f64 / test_trades as f64 * 100.0
    };
    println!("test_win_rate: {:.2}%", test_win_rate);
    println!("test_total_pnl: {:.2}", test_pnl);
    println!(
        "test_avg_trade_pnl: {:.2}",
        if test_trades == 0 {
            0.0
        } else {
            test_pnl / test_trades as f64
        }
    );
    println!("test_drawdown_sum: {:.2}", test_dd_sum);
    if let Some(path) = &opts.output_csv {
        println!("folds_csv: {path}");
    }
    Ok(())
}

fn configured_single_pair(
    mut cfg: AppConfig,
    pair_id: &str,
    a: &str,
    b: &str,
    beta: f64,
    leg_notional: f64,
    params: Option<ParamSet>,
) -> AppConfig {
    if let Some(params) = params {
        cfg.strategy.rolling_window_ticks = params.window;
        cfg.strategy.min_samples = params.min_samples;
        cfg.strategy.entry_zscore = params.entry_zscore;
        cfg.strategy.exit_zscore = params.exit_zscore;
        cfg.risk.max_holding_minutes = params.max_holding_bars;
    }
    cfg.pairs = vec![PairConfig {
        id: pair_id.to_string(),
        a: a.to_ascii_uppercase(),
        b: b.to_ascii_uppercase(),
        enabled: true,
        beta: Some(beta),
        leg_notional,
        rolling_window_ticks: params.map(|p| p.window),
        entry_zscore: params.map(|p| p.entry_zscore),
        exit_zscore: params.map(|p| p.exit_zscore),
        min_samples: params.map(|p| p.min_samples),
        max_spread_bps: None,
        use_rolling_beta: params.and_then(|p| p.use_rolling_beta),
        beta_min: params.and_then(|p| p.beta_min),
        beta_max: params.and_then(|p| p.beta_max),
        min_correlation: params.and_then(|p| p.min_correlation),
        max_spread_std_bps: params.and_then(|p| p.max_spread_std_bps),
        entry_confirmation_bars: params.and_then(|p| p.entry_confirmation_bars),
        min_expected_spread_move_after_costs: params
            .and_then(|p| p.min_expected_spread_move_after_costs),
        adverse_zscore: params.and_then(|p| p.adverse_zscore),
        stop_loss_dollars: params.and_then(|p| p.stop_loss_dollars),
        profit_protection_min_dollars: params.and_then(|p| p.profit_protection_min_dollars),
        profit_protection_retrace_fraction: params
            .and_then(|p| p.profit_protection_retrace_fraction),
        profit_protection_floor_dollars: params.and_then(|p| p.profit_protection_floor_dollars),
        min_half_life_ticks: None,
        max_holding_bars: params.map(|p| p.max_holding_bars),
        signal_timeframe: None,
    }];
    cfg
}

fn default_walk_forward_grid() -> Vec<ParamSet> {
    let windows = [390usize];
    let entries = [1.75, 2.00];
    let exits = [0.50];
    let holding_multipliers = [0.5];
    let mut out = Vec::new();
    for window in windows {
        for entry_zscore in entries {
            for exit_zscore in exits {
                for holding_multiplier in holding_multipliers {
                    out.push(ParamSet {
                        window,
                        min_samples: ((window as f64) * 0.75).round() as usize,
                        entry_zscore,
                        exit_zscore,
                        max_holding_bars: ((window as f64) * holding_multiplier).round() as u64,
                        use_rolling_beta: None,
                        beta_min: None,
                        beta_max: None,
                        min_correlation: None,
                        max_spread_std_bps: None,
                        entry_confirmation_bars: None,
                        min_expected_spread_move_after_costs: None,
                        adverse_zscore: None,
                        stop_loss_dollars: None,
                        profit_protection_min_dollars: None,
                        profit_protection_retrace_fraction: None,
                        profit_protection_floor_dollars: None,
                    });
                }
            }
        }
    }
    out
}

fn walk_forward_score(report: &SimReport, min_trades: usize) -> f64 {
    let missing_trade_penalty = min_trades.saturating_sub(report.trades.len()) as f64 * 1_000.0;
    report.total_pnl() - report.max_drawdown() - missing_trade_penalty
}

struct ReportContext {
    source: String,
    timeframe: String,
    feed: Option<String>,
    requested_from: Option<DateTime<Utc>>,
    requested_to: Option<DateTime<Utc>>,
}

async fn run_loaded_bars(
    cfg: AppConfig,
    report: ReportContext,
    slippage_bps: f64,
    trades_csv: Option<String>,
    report_dir: Option<String>,
    bars: HashMap<String, Vec<crate::alpaca::Bar>>,
) -> Result<()> {
    let sim = simulate_loaded_bars(cfg.clone(), slippage_bps, bars.clone()).await?;
    if let Some(path) = &trades_csv {
        write_trades_csv(path, &sim.trades)?;
    }
    if let Some(report_dir) = &report_dir {
        write_backtest_report_dir(
            report_dir,
            &cfg,
            &report,
            slippage_bps,
            &sim,
            &bars,
            &trades_csv,
        )
        .await?;
    }
    print_report(
        &cfg,
        &report,
        slippage_bps,
        trades_csv.as_deref(),
        sim.data_start,
        sim.data_end,
        sim.bars,
        &sim.trades,
        &sim.equity_curve,
    );
    if let Some(path) = &report_dir {
        println!("report_dir: {path}");
    }
    Ok(())
}

async fn simulate_loaded_bars(
    cfg: AppConfig,
    slippage_bps: f64,
    bars: HashMap<String, Vec<crate::alpaca::Bar>>,
) -> Result<SimReport> {
    let timeline = aligned_timeline(&bars);
    if timeline.is_empty() {
        return Err(anyhow!(
            "no overlapping bars returned for configured symbols"
        ));
    }

    let mut pair_engine = PairEngine::new(&cfg);
    let mut risk = RiskEngine::new(cfg.risk.clone());
    let mut positions: HashMap<String, SimPosition> = HashMap::new();
    let mut trades = Vec::new();
    let mut equity_curve = Vec::new();

    for (idx, ts) in timeline.iter().enumerate() {
        let quotes = quotes_at(*ts, &bars)?;
        let decisions = pair_engine.on_quotes(&quotes, &risk).await;
        for decision in decisions {
            if should_force_time_exit(&positions, &decision, idx, cfg.risk.max_holding_minutes) {
                close_position(
                    &mut positions,
                    &mut trades,
                    &mut risk,
                    &decision,
                    &quotes,
                    idx,
                    slippage_bps,
                    "max_holding_bars",
                )?;
                continue;
            }
            if let Some(stop_reason) =
                forced_stop_reason(&mut positions, &decision, &quotes, slippage_bps)?
            {
                close_position(
                    &mut positions,
                    &mut trades,
                    &mut risk,
                    &decision,
                    &quotes,
                    idx,
                    slippage_bps,
                    stop_reason,
                )?;
                continue;
            }

            match decision.action {
                DecisionAction::EnterLongSpread | DecisionAction::EnterShortSpread => {
                    if risk.allow_decision(&decision) {
                        open_position(
                            &mut positions,
                            &mut risk,
                            &decision,
                            &quotes,
                            idx,
                            slippage_bps,
                        )?;
                    }
                }
                DecisionAction::Exit => {
                    close_position(
                        &mut positions,
                        &mut trades,
                        &mut risk,
                        &decision,
                        &quotes,
                        idx,
                        slippage_bps,
                        "zscore_converged",
                    )?;
                }
                _ => {}
            }
        }
        equity_curve.push(trades.iter().map(|t: &SimTrade| t.pnl).sum::<f64>());
    }

    if let Some(last_ts) = timeline.last().copied() {
        let quotes = quotes_at(last_ts, &bars)?;
        let open_pair_ids: Vec<_> = positions.keys().cloned().collect();
        for pair_id in open_pair_ids {
            let Some(pair) = cfg.pairs.iter().find(|p| p.id == pair_id) else {
                continue;
            };
            let decision = PairDecision {
                ts: last_ts,
                pair_id: pair.id.clone(),
                a: pair.a.clone(),
                b: pair.b.clone(),
                beta: pair.beta.unwrap_or(1.0),
                spread: 0.0,
                mean: 0.0,
                std: 0.0,
                z_score: 0.0,
                correlation: 0.0,
                action: DecisionAction::Exit,
                reason: "end of backtest".into(),
                leg_notional: pair.leg_notional,
                max_holding_bars: pair.max_holding_bars,
                adverse_zscore: pair.adverse_zscore,
                stop_loss_dollars: pair.stop_loss_dollars,
                profit_protection_min_dollars: pair.profit_protection_min_dollars,
                profit_protection_retrace_fraction: pair.profit_protection_retrace_fraction,
                profit_protection_floor_dollars: pair.profit_protection_floor_dollars,
            };
            close_position(
                &mut positions,
                &mut trades,
                &mut risk,
                &decision,
                &quotes,
                timeline.len(),
                slippage_bps,
                "end_of_backtest",
            )?;
        }
        let realised = trades.iter().map(|t: &SimTrade| t.pnl).sum::<f64>();
        if let Some(last) = equity_curve.last_mut() {
            *last = realised;
        } else {
            equity_curve.push(realised);
        }
    }

    Ok(SimReport {
        data_start: timeline.first().copied(),
        data_end: timeline.last().copied(),
        bars: timeline.len(),
        trades,
        equity_curve,
    })
}

fn open_position(
    positions: &mut HashMap<String, SimPosition>,
    risk: &mut RiskEngine,
    decision: &PairDecision,
    quotes: &HashMap<String, Quote>,
    idx: usize,
    slippage_bps: f64,
) -> Result<()> {
    if positions.contains_key(&decision.pair_id) {
        return Ok(());
    }

    let qa = quotes
        .get(&decision.a)
        .ok_or_else(|| anyhow!("missing quote for {}", decision.a))?;
    let qb = quotes
        .get(&decision.b)
        .ok_or_else(|| anyhow!("missing quote for {}", decision.b))?;
    let (qty_a, qty_b) = balanced_quantities(decision.leg_notional, qa.mid(), qb.mid());
    let (side_a, side_b) = match decision.action {
        DecisionAction::EnterLongSpread => (Side::Buy, Side::Sell),
        DecisionAction::EnterShortSpread => (Side::Sell, Side::Buy),
        _ => return Ok(()),
    };

    positions.insert(
        decision.pair_id.clone(),
        SimPosition {
            pair_id: decision.pair_id.clone(),
            opened_at: decision.ts,
            opened_index: idx,
            a: SimLeg {
                symbol: decision.a.clone(),
                qty: qty_a,
                side: side_a,
                entry_price: fill_price(qa.mid(), side_a, true, slippage_bps),
            },
            b: SimLeg {
                symbol: decision.b.clone(),
                qty: qty_b,
                side: side_b,
                entry_price: fill_price(qb.mid(), side_b, true, slippage_bps),
            },
            entry_z: decision.z_score,
            max_open_pnl: 0.0,
        },
    );
    risk.mark_open(&decision.pair_id);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn close_position(
    positions: &mut HashMap<String, SimPosition>,
    trades: &mut Vec<SimTrade>,
    risk: &mut RiskEngine,
    decision: &PairDecision,
    quotes: &HashMap<String, Quote>,
    idx: usize,
    slippage_bps: f64,
    reason: &str,
) -> Result<()> {
    let Some(position) = positions.remove(&decision.pair_id) else {
        return Ok(());
    };
    let qa = quotes
        .get(&position.a.symbol)
        .ok_or_else(|| anyhow!("missing quote for {}", position.a.symbol))?;
    let qb = quotes
        .get(&position.b.symbol)
        .ok_or_else(|| anyhow!("missing quote for {}", position.b.symbol))?;
    let exit_a = fill_price(qa.mid(), position.a.side.opposite(), true, slippage_bps);
    let exit_b = fill_price(qb.mid(), position.b.side.opposite(), true, slippage_bps);
    let pnl_a = leg_pnl(&position.a, exit_a);
    let pnl_b = leg_pnl(&position.b, exit_b);
    let pnl = pnl_a + pnl_b;
    let estimated_costs = estimated_slippage_cost(&position.a, exit_a, slippage_bps)
        + estimated_slippage_cost(&position.b, exit_b, slippage_bps);

    trades.push(SimTrade {
        pair_id: position.pair_id.clone(),
        opened_at: position.opened_at,
        closed_at: decision.ts,
        opened_index: position.opened_index,
        closed_index: idx,
        symbol_a: position.a.symbol.clone(),
        symbol_b: position.b.symbol.clone(),
        side_a: position.a.side,
        side_b: position.b.side,
        holding_bars: idx.saturating_sub(position.opened_index),
        qty_a: position.a.qty,
        qty_b: position.b.qty,
        entry_price_a: position.a.entry_price,
        entry_price_b: position.b.entry_price,
        exit_price_a: exit_a,
        exit_price_b: exit_b,
        entry_z: position.entry_z,
        exit_z: decision.z_score,
        pnl_a,
        pnl_b,
        pnl,
        estimated_costs,
        reason: reason.into(),
    });
    risk.mark_closed(&decision.pair_id);
    Ok(())
}

fn leg_pnl(leg: &SimLeg, exit_price: f64) -> f64 {
    match leg.side {
        Side::Buy => (exit_price - leg.entry_price) * leg.qty,
        Side::Sell => (leg.entry_price - exit_price) * leg.qty,
    }
}

fn estimated_slippage_cost(leg: &SimLeg, exit_price: f64, slippage_bps: f64) -> f64 {
    let entry_cost = leg.entry_price * leg.qty * slippage_bps / 10_000.0;
    let exit_cost = exit_price * leg.qty * slippage_bps / 10_000.0;
    entry_cost + exit_cost
}

fn fill_price(mid: f64, side: Side, apply_slippage: bool, slippage_bps: f64) -> f64 {
    if !apply_slippage {
        return mid;
    }
    let slip = slippage_bps / 10_000.0;
    match side {
        Side::Buy => mid * (1.0 + slip),
        Side::Sell => mid * (1.0 - slip),
    }
}

fn should_force_time_exit(
    positions: &HashMap<String, SimPosition>,
    decision: &PairDecision,
    idx: usize,
    max_holding_bars: u64,
) -> bool {
    let max_holding_bars = decision.max_holding_bars.unwrap_or(max_holding_bars);
    positions
        .get(&decision.pair_id)
        .map(|p| idx.saturating_sub(p.opened_index) >= max_holding_bars as usize)
        .unwrap_or(false)
}

fn forced_stop_reason(
    positions: &mut HashMap<String, SimPosition>,
    decision: &PairDecision,
    quotes: &HashMap<String, Quote>,
    slippage_bps: f64,
) -> Result<Option<&'static str>> {
    let Some(position) = positions.get(&decision.pair_id) else {
        return Ok(None);
    };

    if let Some(adverse_zscore) = decision.adverse_zscore {
        let entry_side = position.entry_z.signum();
        let adverse_move = entry_side * (decision.z_score - position.entry_z);
        if entry_side.abs() > 0.0 && adverse_move >= adverse_zscore {
            return Ok(Some("risk_stop"));
        }
    }

    let current_pnl = unrealized_pnl(position, quotes, slippage_bps)?;
    if let Some(stop_loss_dollars) = decision.stop_loss_dollars {
        if current_pnl <= -stop_loss_dollars.abs() {
            return Ok(Some("risk_stop"));
        }
    }

    if let Some(position) = positions.get_mut(&decision.pair_id) {
        position.max_open_pnl = position.max_open_pnl.max(current_pnl);
        if let Some(min_profit) = decision.profit_protection_min_dollars {
            let retrace_fraction = decision
                .profit_protection_retrace_fraction
                .unwrap_or(0.50)
                .clamp(0.0, 1.0);
            let floor = decision
                .profit_protection_floor_dollars
                .unwrap_or(0.0)
                .max(0.0);
            let trigger = (position.max_open_pnl * retrace_fraction).max(floor);
            if position.max_open_pnl >= min_profit.max(0.0) && current_pnl <= trigger {
                return Ok(Some("profit_protection"));
            }
        }
    }

    Ok(None)
}

fn unrealized_pnl(
    position: &SimPosition,
    quotes: &HashMap<String, Quote>,
    slippage_bps: f64,
) -> Result<f64> {
    let qa = quotes
        .get(&position.a.symbol)
        .ok_or_else(|| anyhow!("missing quote for {}", position.a.symbol))?;
    let qb = quotes
        .get(&position.b.symbol)
        .ok_or_else(|| anyhow!("missing quote for {}", position.b.symbol))?;
    let exit_a = fill_price(qa.mid(), position.a.side.opposite(), true, slippage_bps);
    let exit_b = fill_price(qb.mid(), position.b.side.opposite(), true, slippage_bps);
    Ok(leg_pnl(&position.a, exit_a) + leg_pnl(&position.b, exit_b))
}

fn parse_start(raw: &str) -> Result<DateTime<Utc>> {
    Ok(NaiveDate::parse_from_str(raw, "%Y-%m-%d")?
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| anyhow!("invalid start date"))?
        .and_utc())
}

fn parse_end(raw: &str) -> Result<DateTime<Utc>> {
    Ok(NaiveDate::parse_from_str(raw, "%Y-%m-%d")?
        .and_hms_opt(23, 59, 59)
        .ok_or_else(|| anyhow!("invalid end date"))?
        .and_utc())
}

fn aligned_timeline(bars: &HashMap<String, Vec<crate::alpaca::Bar>>) -> Vec<DateTime<Utc>> {
    let mut iter = bars.values();
    let Some(first) = iter.next() else {
        return Vec::new();
    };
    let mut set: BTreeSet<_> = first.iter().map(|b| b.t).collect();
    for series in iter {
        let other: BTreeSet<_> = series.iter().map(|b| b.t).collect();
        set = set.intersection(&other).copied().collect();
    }
    set.into_iter().collect()
}

fn quotes_at(
    ts: DateTime<Utc>,
    bars: &HashMap<String, Vec<crate::alpaca::Bar>>,
) -> Result<HashMap<String, Quote>> {
    let mut out = HashMap::new();
    for (symbol, series) in bars {
        let Some(bar) = series.iter().find(|b| b.t == ts) else {
            return Err(anyhow!("missing aligned bar for {symbol} at {ts}"));
        };
        out.insert(
            symbol.clone(),
            Quote {
                symbol: symbol.clone(),
                bid: bar.c - 0.01,
                ask: bar.c + 0.01,
                bid_size: 100.0,
                ask_size: 100.0,
                ts,
            },
        );
    }
    Ok(out)
}

fn write_trades_csv(path: &str, trades: &[SimTrade]) -> Result<()> {
    let mut out = "pair_id,opened_at,closed_at,opened_index,closed_index,holding_bars,symbol_a,symbol_b,side_a,side_b,qty_a,qty_b,entry_price_a,entry_price_b,exit_price_a,exit_price_b,entry_z,exit_z,pnl_a,pnl_b,pnl,estimated_costs,reason\n"
        .to_string();
    for t in trades {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{:?},{:?},{:.0},{:.0},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.2},{:.2},{:.2},{:.2},{}\n",
            t.pair_id,
            t.opened_at,
            t.closed_at,
            t.opened_index,
            t.closed_index,
            t.holding_bars,
            t.symbol_a,
            t.symbol_b,
            t.side_a,
            t.side_b,
            t.qty_a,
            t.qty_b,
            t.entry_price_a,
            t.entry_price_b,
            t.exit_price_a,
            t.exit_price_b,
            t.entry_z,
            t.exit_z,
            t.pnl_a,
            t.pnl_b,
            t.pnl,
            t.estimated_costs,
            t.reason
        ));
    }
    fs::write(path, out)?;
    Ok(())
}

async fn write_backtest_report_dir(
    report_dir: &str,
    cfg: &AppConfig,
    report: &ReportContext,
    slippage_bps: f64,
    sim: &SimReport,
    bars: &HashMap<String, Vec<crate::alpaca::Bar>>,
    trades_csv: &Option<String>,
) -> Result<()> {
    analytics::ensure_report_dir(report_dir)?;
    analytics::write_report_file(
        report_dir,
        "README.md",
        &backtest_summary_markdown(cfg, report, slippage_bps, sim, trades_csv),
    )?;
    analytics::write_report_file(
        report_dir,
        "assumptions.md",
        &transaction_cost_assumptions(slippage_bps),
    )?;
    analytics::write_report_file(report_dir, "trades.csv", &trades_csv_body(&sim.trades))?;
    analytics::write_report_file(
        report_dir,
        "pair_summary.csv",
        &pair_summary_csv(&sim.trades),
    )?;
    analytics::write_report_file(
        report_dir,
        "equity_curve.csv",
        &equity_curve_csv(&sim.equity_curve),
    )?;
    analytics::write_report_file(
        report_dir,
        "drawdown_curve.csv",
        &drawdown_curve_csv(&sim.equity_curve),
    )?;
    analytics::write_report_file(
        report_dir,
        "markouts.csv",
        &markouts_csv(&sim.trades, bars, slippage_bps)?,
    )?;
    analytics::write_report_file(
        report_dir,
        "slippage_sensitivity.csv",
        &slippage_sensitivity_csv(cfg.clone(), bars.clone(), slippage_bps).await?,
    )?;
    Ok(())
}

fn backtest_summary_markdown(
    cfg: &AppConfig,
    report: &ReportContext,
    slippage_bps: f64,
    sim: &SimReport,
    trades_csv: &Option<String>,
) -> String {
    let pair_list = cfg
        .pairs
        .iter()
        .filter(|p| p.enabled)
        .map(|p| p.id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let mut out = String::new();
    out.push_str("# Backtest Analytics Report\n\n");
    out.push_str("This directory is generated by `cargo run -- backtest... --report-dir <dir>` or `cargo run -- backtest-csv... --report-dir <dir>`.\n\n");
    out.push_str("## Scope\n\n");
    out.push_str(&format!("- source: `{}`\n", report.source));
    out.push_str(&format!("- pairs: `{}`\n", pair_list));
    out.push_str(&format!("- timeframe: `{}`\n", report.timeframe));
    if let Some(feed) = &report.feed {
        out.push_str(&format!("- feed: `{feed}`\n"));
    }
    if let (Some(from), Some(to)) = (sim.data_start, sim.data_end) {
        out.push_str(&format!(
            "- observed data range: `{}` to `{}`\n",
            from.date_naive(),
            to.date_naive()
        ));
    }
    out.push_str(&format!("- aligned bars: `{}`\n", sim.bars));
    out.push_str(&format!(
        "- slippage assumption: `{:.2}` bps per fill\n",
        slippage_bps
    ));
    if let Some(path) = trades_csv {
        out.push_str(&format!("- extra trades CSV: `{path}`\n"));
    }
    out.push_str("\n## Results\n\n");
    out.push_str(&format!("- trades: `{}`\n", sim.trades.len()));
    out.push_str(&format!("- wins: `{}`\n", sim.wins()));
    out.push_str(&format!("- win rate: `{:.2}%`\n", sim.win_rate()));
    out.push_str(&format!("- total PnL: `{:.2}`\n", sim.total_pnl()));
    out.push_str(&format!(
        "- average trade PnL: `{:.2}`\n",
        sim.avg_trade_pnl()
    ));
    out.push_str(&format!("- max drawdown: `{:.2}`\n", sim.max_drawdown()));
    out.push_str("\n## Files\n\n");
    out.push_str("- `trades.csv`: trade-level PnL attribution by leg.\n");
    out.push_str("- `pair_summary.csv`: per-pair trade count, win rate, PnL, average PnL, and max realised drawdown.\n");
    out.push_str("- `equity_curve.csv`: realised cumulative PnL by aligned bar index.\n");
    out.push_str("- `drawdown_curve.csv`: realised drawdown by aligned bar index.\n");
    out.push_str("- `markouts.csv`: post-entry markouts at 1, 5, and 20 bars using the same exit-cost convention as the backtest.\n");
    out.push_str("- `slippage_sensitivity.csv`: same strategy rerun at alternative per-fill slippage assumptions.\n");
    out.push_str("- `assumptions.md`: transaction cost and data assumptions.\n");
    out
}

fn transaction_cost_assumptions(slippage_bps: f64) -> String {
    format!(
        "# Transaction Cost Assumptions\n\n- Slippage is modeled as `{:.2}` bps per fill.\n- Buy fills are marked up from mid; sell fills are marked down from mid.\n- Each paired trade has four fills: two entry legs and two exit legs.\n- Broker commissions, borrow fees, SEC/TAF fees, rejected orders, partial fills, and queue position are not modeled in the CSV backtester.\n- Live/paper execution has additional risks from legging, stale quotes, borrow availability, order cancellation, and broker/API outages.\n",
        slippage_bps
    )
}

fn trades_csv_body(trades: &[SimTrade]) -> String {
    let mut out = "pair_id,opened_at,closed_at,opened_index,closed_index,holding_bars,symbol_a,symbol_b,side_a,side_b,qty_a,qty_b,entry_price_a,entry_price_b,exit_price_a,exit_price_b,entry_z,exit_z,pnl_a,pnl_b,pnl,estimated_costs,reason\n".to_string();
    for t in trades {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{:?},{:?},{:.0},{:.0},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.2},{:.2},{:.2},{:.2},{}\n",
            t.pair_id, t.opened_at, t.closed_at, t.opened_index, t.closed_index,
            t.holding_bars, t.symbol_a, t.symbol_b, t.side_a, t.side_b, t.qty_a, t.qty_b,
            t.entry_price_a, t.entry_price_b, t.exit_price_a, t.exit_price_b, t.entry_z,
            t.exit_z, t.pnl_a, t.pnl_b, t.pnl, t.estimated_costs, t.reason
        ));
    }
    out
}

fn pair_summary_csv(trades: &[SimTrade]) -> String {
    let mut by_pair: HashMap<&str, Vec<&SimTrade>> = HashMap::new();
    for trade in trades {
        by_pair.entry(&trade.pair_id).or_default().push(trade);
    }
    let mut rows: Vec<_> = by_pair.into_iter().collect();
    rows.sort_by_key(|(pair_id, _)| *pair_id);
    let mut out =
        "pair_id,trades,wins,win_rate,total_pnl,avg_trade_pnl,max_realised_drawdown\n".to_string();
    for (pair_id, pair_trades) in rows {
        let trades_count = pair_trades.len();
        let wins = pair_trades.iter().filter(|t| t.pnl > 0.0).count();
        let total_pnl = pair_trades.iter().map(|t| t.pnl).sum::<f64>();
        let avg = if trades_count == 0 {
            0.0
        } else {
            total_pnl / trades_count as f64
        };
        let win_rate = if trades_count == 0 {
            0.0
        } else {
            wins as f64 / trades_count as f64 * 100.0
        };
        let mut equity = Vec::new();
        let mut realised = 0.0;
        for trade in pair_trades {
            realised += trade.pnl;
            equity.push(realised);
        }
        out.push_str(&format!(
            "{},{},{},{:.2},{:.2},{:.2},{:.2}\n",
            pair_id,
            trades_count,
            wins,
            win_rate,
            total_pnl,
            avg,
            max_drawdown(&equity)
        ));
    }
    out
}

fn equity_curve_csv(equity: &[f64]) -> String {
    let mut out = "bar_index,equity\n".to_string();
    for (idx, value) in equity.iter().enumerate() {
        out.push_str(&format!("{},{:.2}\n", idx, value));
    }
    out
}

fn drawdown_curve_csv(equity: &[f64]) -> String {
    let mut out = "bar_index,equity,peak,drawdown\n".to_string();
    let mut peak = 0.0;
    for (idx, value) in equity.iter().enumerate() {
        if *value > peak {
            peak = *value;
        }
        out.push_str(&format!(
            "{},{:.2},{:.2},{:.2}\n",
            idx,
            value,
            peak,
            peak - value
        ));
    }
    out
}

fn markouts_csv(
    trades: &[SimTrade],
    bars: &HashMap<String, Vec<crate::alpaca::Bar>>,
    slippage_bps: f64,
) -> Result<String> {
    let horizons = [1usize, 5, 20];
    let mut out =
        "pair_id,event_type,event_at,horizon_bars,markout_pnl,markout_pnl_a,markout_pnl_b\n"
            .to_string();
    for trade in trades {
        for horizon in horizons {
            let idx = trade.opened_index + horizon;
            if let Some((pnl_a, pnl_b)) = markout_at(trade, bars, idx, slippage_bps)? {
                out.push_str(&format!(
                    "{},signal_order_entry,{},{},{:.2},{:.2},{:.2}\n",
                    trade.pair_id,
                    trade.opened_at,
                    horizon,
                    pnl_a + pnl_b,
                    pnl_a,
                    pnl_b
                ));
            }
        }
    }
    Ok(out)
}

fn markout_at(
    trade: &SimTrade,
    bars: &HashMap<String, Vec<crate::alpaca::Bar>>,
    idx: usize,
    slippage_bps: f64,
) -> Result<Option<(f64, f64)>> {
    let Some(a_bar) = bars.get(&trade.symbol_a).and_then(|series| series.get(idx)) else {
        return Ok(None);
    };
    let Some(b_bar) = bars.get(&trade.symbol_b).and_then(|series| series.get(idx)) else {
        return Ok(None);
    };
    let a_leg = SimLeg {
        symbol: trade.symbol_a.clone(),
        qty: trade.qty_a,
        side: trade.side_a,
        entry_price: trade.entry_price_a,
    };
    let b_leg = SimLeg {
        symbol: trade.symbol_b.clone(),
        qty: trade.qty_b,
        side: trade.side_b,
        entry_price: trade.entry_price_b,
    };
    let exit_a = fill_price(a_bar.c, trade.side_a.opposite(), true, slippage_bps);
    let exit_b = fill_price(b_bar.c, trade.side_b.opposite(), true, slippage_bps);
    Ok(Some((leg_pnl(&a_leg, exit_a), leg_pnl(&b_leg, exit_b))))
}

async fn slippage_sensitivity_csv(
    cfg: AppConfig,
    bars: HashMap<String, Vec<crate::alpaca::Bar>>,
    base_slippage_bps: f64,
) -> Result<String> {
    let mut candidates = vec![0.0, base_slippage_bps, 2.0, 5.0, 10.0];
    candidates.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    candidates.dedup_by(|a, b| (*a - *b).abs() < f64::EPSILON);
    let mut out =
        "slippage_bps,trades,wins,win_rate,total_pnl,avg_trade_pnl,max_drawdown\n".to_string();
    for slippage in candidates {
        let sim = simulate_loaded_bars(cfg.clone(), slippage, bars.clone()).await?;
        out.push_str(&format!(
            "{:.2},{},{},{:.2},{:.2},{:.2},{:.2}\n",
            slippage,
            sim.trades.len(),
            sim.wins(),
            sim.win_rate(),
            sim.total_pnl(),
            sim.avg_trade_pnl(),
            sim.max_drawdown()
        ));
    }
    Ok(out)
}

fn write_walk_forward_csv(path: &str, rows: &[WalkForwardRow]) -> Result<()> {
    let mut out = "fold,train_start,train_end,test_start,test_end,window,min_samples,entry_zscore,exit_zscore,max_holding_bars,train_trades,train_win_rate,train_pnl,train_max_drawdown,test_trades,test_win_rate,test_pnl,test_avg_trade_pnl,test_max_drawdown\n".to_string();
    for row in rows {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{:.2},{:.2},{},{},{:.2},{:.2},{:.2},{},{:.2},{:.2},{:.2},{:.2}\n",
            row.fold,
            row.train_start,
            row.train_end,
            row.test_start,
            row.test_end,
            row.params.window,
            row.params.min_samples,
            row.params.entry_zscore,
            row.params.exit_zscore,
            row.params.max_holding_bars,
            row.train.trades.len(),
            row.train.win_rate(),
            row.train.total_pnl(),
            row.train.max_drawdown(),
            row.test.trades.len(),
            row.test.win_rate(),
            row.test.total_pnl(),
            row.test.avg_trade_pnl(),
            row.test.max_drawdown(),
        ));
    }
    fs::write(path, out)?;
    Ok(())
}

fn write_walk_forward_report_dir(
    report_dir: &str,
    opts: &WalkForwardCsvOptions,
    timeline: &[DateTime<Utc>],
    rows: &[WalkForwardRow],
) -> Result<()> {
    analytics::ensure_report_dir(report_dir)?;
    analytics::write_report_file(
        report_dir,
        "README.md",
        &walk_forward_summary_markdown(opts, timeline, rows),
    )?;
    analytics::write_report_file(report_dir, "folds.csv", &walk_forward_csv_body(rows))?;
    analytics::write_report_file(
        report_dir,
        "test_pair_summary.csv",
        &walk_forward_pair_csv(rows),
    )?;
    analytics::write_report_file(
        report_dir,
        "out_of_sample_split.md",
        &out_of_sample_split_markdown(opts, timeline, rows),
    )?;
    Ok(())
}

fn walk_forward_csv_body(rows: &[WalkForwardRow]) -> String {
    let mut out = "fold,train_start,train_end,test_start,test_end,window,min_samples,entry_zscore,exit_zscore,max_holding_bars,train_trades,train_win_rate,train_pnl,train_max_drawdown,test_trades,test_win_rate,test_pnl,test_avg_trade_pnl,test_max_drawdown\n".to_string();
    for row in rows {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{:.2},{:.2},{},{},{:.2},{:.2},{:.2},{},{:.2},{:.2},{:.2},{:.2}\n",
            row.fold,
            row.train_start,
            row.train_end,
            row.test_start,
            row.test_end,
            row.params.window,
            row.params.min_samples,
            row.params.entry_zscore,
            row.params.exit_zscore,
            row.params.max_holding_bars,
            row.train.trades.len(),
            row.train.win_rate(),
            row.train.total_pnl(),
            row.train.max_drawdown(),
            row.test.trades.len(),
            row.test.win_rate(),
            row.test.total_pnl(),
            row.test.avg_trade_pnl(),
            row.test.max_drawdown(),
        ));
    }
    out
}

fn walk_forward_summary_markdown(
    opts: &WalkForwardCsvOptions,
    timeline: &[DateTime<Utc>],
    rows: &[WalkForwardRow],
) -> String {
    let test_trades = rows.iter().map(|r| r.test.trades.len()).sum::<usize>();
    let test_wins = rows.iter().map(|r| r.test.wins()).sum::<usize>();
    let test_pnl = rows.iter().map(|r| r.test.total_pnl()).sum::<f64>();
    let test_avg = if test_trades == 0 {
        0.0
    } else {
        test_pnl / test_trades as f64
    };
    let test_win_rate = if test_trades == 0 {
        0.0
    } else {
        test_wins as f64 / test_trades as f64 * 100.0
    };
    let data_start = timeline
        .first()
        .map(|d| d.date_naive().to_string())
        .unwrap_or_default();
    let data_end = timeline
        .last()
        .map(|d| d.date_naive().to_string())
        .unwrap_or_default();

    format!(
        "# Walk-Forward Report\n\n## Scope\n\n- source: `{}`\n- pair: `{}`\n- symbols: `{}` / `{}`\n- data range: `{}` to `{}`\n- train bars per fold: `{}`\n- test bars per fold: `{}`\n- minimum train trades for scoring: `{}`\n- slippage: `{:.2}` bps per fill\n- folds: `{}`\n\n## Out-of-Sample Result\n\n- test trades: `{}`\n- test wins: `{}`\n- test win rate: `{:.2}%`\n- test total PnL: `{:.2}`\n- test average trade PnL: `{:.2}`\n\n## Files\n\n- `folds.csv`: train/test metrics and selected parameters by fold.\n- `test_pair_summary.csv`: aggregate out-of-sample metrics for the pair.\n- `out_of_sample_split.md`: exact train/test dates for every fold.\n",
        opts.bars_csv,
        opts.pair_id,
        opts.a,
        opts.b,
        data_start,
        data_end,
        opts.train_bars,
        opts.test_bars,
        opts.min_train_trades,
        opts.slippage_bps,
        rows.len(),
        test_trades,
        test_wins,
        test_win_rate,
        test_pnl,
        test_avg
    )
}

fn walk_forward_pair_csv(rows: &[WalkForwardRow]) -> String {
    let test_trades = rows.iter().map(|r| r.test.trades.len()).sum::<usize>();
    let test_wins = rows.iter().map(|r| r.test.wins()).sum::<usize>();
    let test_pnl = rows.iter().map(|r| r.test.total_pnl()).sum::<f64>();
    let test_dd_sum = rows.iter().map(|r| r.test.max_drawdown()).sum::<f64>();
    let win_rate = if test_trades == 0 {
        0.0
    } else {
        test_wins as f64 / test_trades as f64 * 100.0
    };
    let avg = if test_trades == 0 {
        0.0
    } else {
        test_pnl / test_trades as f64
    };
    let pair_id = rows
        .first()
        .and_then(|r| r.test.trades.first().or_else(|| r.train.trades.first()))
        .map(|t| t.pair_id.as_str())
        .unwrap_or("UNKNOWN");
    format!(
        "pair_id,folds,test_trades,test_wins,test_win_rate,test_total_pnl,test_avg_trade_pnl,test_drawdown_sum\n{},{},{},{},{:.2},{:.2},{:.2},{:.2}\n",
        pair_id,
        rows.len(),
        test_trades,
        test_wins,
        win_rate,
        test_pnl,
        avg,
        test_dd_sum
    )
}

fn out_of_sample_split_markdown(
    opts: &WalkForwardCsvOptions,
    timeline: &[DateTime<Utc>],
    rows: &[WalkForwardRow],
) -> String {
    let mut out = "# Out-of-Sample Split\n\n".to_string();
    if let (Some(first), Some(last)) = (timeline.first(), timeline.last()) {
        out.push_str(&format!(
            "The aligned dataset spans `{}` to `{}`. Each fold trains on `{}` aligned bars and evaluates only on the next `{}` unseen aligned bars.\n\n",
            first,
            last,
            opts.train_bars,
            opts.test_bars
        ));
    }
    out.push_str("| fold | train start | train end | test start | test end |\n");
    out.push_str("| ---: | --- | --- | --- | --- |\n");
    for row in rows {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            row.fold, row.train_start, row.train_end, row.test_start, row.test_end
        ));
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn print_report(
    cfg: &AppConfig,
    report: &ReportContext,
    slippage_bps: f64,
    trades_csv: Option<&str>,
    data_start: Option<DateTime<Utc>>,
    data_end: Option<DateTime<Utc>>,
    bars: usize,
    trades: &[SimTrade],
    equity_curve: &[f64],
) {
    let pnl = trades.iter().map(|t| t.pnl).sum::<f64>();
    let wins = trades.iter().filter(|t| t.pnl > 0.0).count();
    let win_rate = if trades.is_empty() {
        0.0
    } else {
        wins as f64 / trades.len() as f64 * 100.0
    };
    let avg_trade = if trades.is_empty() {
        0.0
    } else {
        pnl / trades.len() as f64
    };
    let max_drawdown = max_drawdown(equity_curve);
    let pair_list = cfg
        .pairs
        .iter()
        .filter(|p| p.enabled)
        .map(|p| p.id.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    println!("Backtest report");
    println!("source: {}", report.source);
    println!("pairs: {pair_list}");
    if let (Some(from), Some(to)) = (report.requested_from, report.requested_to) {
        println!(
            "requested_range: {} to {}",
            from.date_naive(),
            to.date_naive()
        );
    }
    if let (Some(data_start), Some(data_end)) = (data_start, data_end) {
        println!(
            "data_range: {} to {}",
            data_start.date_naive(),
            data_end.date_naive()
        );
    }
    println!("timeframe: {}", report.timeframe);
    if let Some(feed) = &report.feed {
        println!("feed: {feed}");
    }
    println!("bars: {bars}");
    println!("slippage_bps per fill: {:.2}", slippage_bps);
    println!("trades: {}", trades.len());
    println!("wins: {wins}");
    println!("win_rate: {:.2}%", win_rate);
    println!("total_pnl: {:.2}", pnl);
    println!("avg_trade_pnl: {:.2}", avg_trade);
    println!("max_drawdown: {:.2}", max_drawdown);
    if let Some(path) = trades_csv {
        println!("trades_csv: {path}");
    }
}

fn max_drawdown(equity: &[f64]) -> f64 {
    let mut peak = 0.0;
    let mut max_dd = 0.0;
    for value in equity {
        if *value > peak {
            peak = *value;
        }
        let dd = peak - value;
        if dd > max_dd {
            max_dd = dd;
        }
    }
    max_dd
}

fn slice_bars(
    bars: &HashMap<String, Vec<crate::alpaca::Bar>>,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> HashMap<String, Vec<crate::alpaca::Bar>> {
    bars.iter()
        .map(|(symbol, series)| {
            (
                symbol.clone(),
                series
                    .iter()
                    .filter(|bar| bar.t >= start && bar.t <= end)
                    .cloned()
                    .collect(),
            )
        })
        .collect()
}

fn read_bars_csv(
    path: &str,
    needed_symbols: &[String],
) -> Result<HashMap<String, Vec<crate::alpaca::Bar>>> {
    let needed: BTreeSet<_> = needed_symbols.iter().map(|s| s.as_str()).collect();
    let raw = fs::read_to_string(path)?;
    let mut out: HashMap<String, Vec<crate::alpaca::Bar>> = HashMap::new();

    for (line_no, line) in raw.lines().enumerate() {
        if line_no == 0 || line.trim().is_empty() {
            continue;
        }
        let cols: Vec<_> = line.split(',').collect();
        if cols.len() < 7 {
            return Err(anyhow!("invalid CSV row {} in {}", line_no + 1, path));
        }
        let symbol = cols[0].trim().to_ascii_uppercase();
        if !needed.contains(symbol.as_str()) {
            continue;
        }
        let t = DateTime::parse_from_rfc3339(cols[1].trim())?.with_timezone(&Utc);
        let bar = crate::alpaca::Bar {
            t,
            o: cols[2].trim().parse()?,
            h: cols[3].trim().parse()?,
            l: cols[4].trim().parse()?,
            c: cols[5].trim().parse()?,
            v: cols[6].trim().parse()?,
            n: optional_parse(cols.get(7).copied())?,
            vw: optional_parse(cols.get(8).copied())?,
        };
        out.entry(symbol).or_default().push(bar);
    }

    for symbol in needed_symbols {
        if !out.contains_key(symbol) {
            return Err(anyhow!("{} was not found in {}", symbol, path));
        }
    }
    for bars in out.values_mut() {
        bars.sort_by_key(|bar| bar.t);
    }
    Ok(out)
}

fn optional_parse<T>(raw: Option<&str>) -> Result<Option<T>>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    Ok(Some(raw.parse()?))
}
