mod alpaca;
mod analytics;
mod backtest;
mod bar_aggregator;
mod bar_poller;
mod config;
mod data_download;
mod execution;
mod pairs;
mod risk;
mod storage;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::time::Duration;
use tokio::time::MissedTickBehavior;
use tracing::{error, info, warn};

use crate::alpaca::{AlpacaClient, BrokerGateway, MockGateway};
use crate::backtest::{BacktestCsvOptions, BacktestOptions, WalkForwardCsvOptions};
use crate::bar_aggregator::BarAggregator;
use crate::bar_poller::AlpacaBarPoller;
use crate::config::AppConfig;
use crate::data_download::DownloadBarsOptions;
use crate::execution::{ExecutionOutcome, PairExecutor};
use crate::pairs::PairEngine;
use crate::risk::RiskEngine;
use crate::storage::AuditStore;

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Cli {
    #[arg(short, long, default_value = "config/default.toml")]
    config: String,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run with synthetic quotes. Useful before connecting to Alpaca.
    Sim,
    /// Run against Alpaca paper or live endpoints depending on config/env.
    Run,
    /// Check ETF assets are tradable, marginable, shortable and easy-to-borrow.
    CheckAssets,
    /// Backtest the configured pairs using Alpaca historical bars.
    Backtest {
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        #[arg(long, default_value = "1Day")]
        timeframe: String,
        #[arg(long, default_value = "iex")]
        feed: String,
        #[arg(long, default_value_t = 10_000)]
        limit: usize,
        #[arg(long, default_value_t = 1.0)]
        slippage_bps: f64,
        #[arg(long)]
        trades_csv: Option<String>,
        #[arg(long)]
        report_dir: Option<String>,
    },
    /// Backtest one pair from a local bars CSV created by download-bars.
    BacktestCsv {
        #[arg(long)]
        bars_csv: String,
        #[arg(long)]
        pair_id: String,
        #[arg(long)]
        a: String,
        #[arg(long)]
        b: String,
        #[arg(long)]
        beta: f64,
        #[arg(long, default_value_t = 60)]
        window: usize,
        #[arg(long, default_value_t = 45)]
        min_samples: usize,
        #[arg(long, default_value_t = 1.75)]
        entry_zscore: f64,
        #[arg(long, default_value_t = 0.50)]
        exit_zscore: f64,
        #[arg(long, default_value_t = 60)]
        max_holding_bars: u64,
        #[arg(long)]
        use_rolling_beta: Option<bool>,
        #[arg(long)]
        beta_min: Option<f64>,
        #[arg(long)]
        beta_max: Option<f64>,
        #[arg(long)]
        min_correlation: Option<f64>,
        #[arg(long)]
        max_spread_std_bps: Option<f64>,
        #[arg(long)]
        entry_confirmation_bars: Option<u64>,
        #[arg(long)]
        min_expected_spread_move_after_costs: Option<f64>,
        #[arg(long)]
        adverse_zscore: Option<f64>,
        #[arg(long)]
        stop_loss_dollars: Option<f64>,
        #[arg(long)]
        profit_protection_min_dollars: Option<f64>,
        #[arg(long)]
        profit_protection_retrace_fraction: Option<f64>,
        #[arg(long)]
        profit_protection_floor_dollars: Option<f64>,
        #[arg(long, default_value_t = 2000.0)]
        leg_notional: f64,
        #[arg(long, default_value_t = 1.0)]
        slippage_bps: f64,
        #[arg(long)]
        trades_csv: Option<String>,
        #[arg(long)]
        report_dir: Option<String>,
    },
    /// Walk-forward optimize on one CSV window and test on the next unseen window.
    WalkForwardCsv {
        #[arg(long)]
        bars_csv: String,
        #[arg(long)]
        pair_id: String,
        #[arg(long)]
        a: String,
        #[arg(long)]
        b: String,
        #[arg(long)]
        beta: f64,
        #[arg(long, default_value_t = 2016)]
        train_bars: usize,
        #[arg(long, default_value_t = 504)]
        test_bars: usize,
        #[arg(long, default_value_t = 5)]
        min_train_trades: usize,
        #[arg(long, default_value_t = 2000.0)]
        leg_notional: f64,
        #[arg(long, default_value_t = 1.0)]
        slippage_bps: f64,
        #[arg(long)]
        output_csv: Option<String>,
        #[arg(long)]
        report_dir: Option<String>,
    },
    /// Download historical bars to CSV.
    DownloadBars {
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        #[arg(long, default_value = "1Min")]
        timeframe: String,
        /// Comma-separated symbols, or "tested" for the ETF universe used in research sweeps.
        #[arg(long, default_value = data_download::tested_symbols())]
        symbols: String,
        #[arg(long, default_value = "iex")]
        feed: String,
        #[arg(long, default_value_t = 10_000)]
        limit: usize,
        #[arg(long, default_value = "data/tested_1min_bars.csv")]
        output: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    if dotenvy::dotenv().is_err() {
        dotenvy::from_path(format!("{}/.env", env!("CARGO_MANIFEST_DIR"))).ok();
    }

    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()))
        .init();

    let cfg = AppConfig::from_file(&cli.config)?;
    match cli.command {
        Commands::Sim => run_engine(cfg, GatewayMode::Mock).await?,
        Commands::Run => run_engine(cfg, GatewayMode::Alpaca).await?,
        Commands::CheckAssets => check_assets(cfg).await?,
        Commands::Backtest {
            from,
            to,
            timeframe,
            feed,
            limit,
            slippage_bps,
            trades_csv,
            report_dir,
        } => {
            backtest::run(
                cfg,
                BacktestOptions {
                    from,
                    to,
                    timeframe,
                    feed,
                    limit,
                    slippage_bps,
                    trades_csv,
                    report_dir,
                },
            )
            .await?
        }
        Commands::BacktestCsv {
            bars_csv,
            pair_id,
            a,
            b,
            beta,
            window,
            min_samples,
            entry_zscore,
            exit_zscore,
            max_holding_bars,
            use_rolling_beta,
            beta_min,
            beta_max,
            min_correlation,
            max_spread_std_bps,
            entry_confirmation_bars,
            min_expected_spread_move_after_costs,
            adverse_zscore,
            stop_loss_dollars,
            profit_protection_min_dollars,
            profit_protection_retrace_fraction,
            profit_protection_floor_dollars,
            leg_notional,
            slippage_bps,
            trades_csv,
            report_dir,
        } => {
            backtest::run_csv(
                cfg,
                BacktestCsvOptions {
                    bars_csv,
                    pair_id,
                    a,
                    b,
                    beta,
                    rolling_window_ticks: window,
                    min_samples,
                    entry_zscore,
                    exit_zscore,
                    max_holding_bars,
                    use_rolling_beta,
                    beta_min,
                    beta_max,
                    min_correlation,
                    max_spread_std_bps,
                    entry_confirmation_bars,
                    min_expected_spread_move_after_costs,
                    adverse_zscore,
                    stop_loss_dollars,
                    profit_protection_min_dollars,
                    profit_protection_retrace_fraction,
                    profit_protection_floor_dollars,
                    leg_notional,
                    slippage_bps,
                    trades_csv,
                    report_dir,
                },
            )
            .await?
        }
        Commands::WalkForwardCsv {
            bars_csv,
            pair_id,
            a,
            b,
            beta,
            train_bars,
            test_bars,
            min_train_trades,
            leg_notional,
            slippage_bps,
            output_csv,
            report_dir,
        } => {
            backtest::run_walk_forward_csv(
                cfg,
                WalkForwardCsvOptions {
                    bars_csv,
                    pair_id,
                    a,
                    b,
                    beta,
                    train_bars,
                    test_bars,
                    min_train_trades,
                    leg_notional,
                    slippage_bps,
                    output_csv,
                    report_dir,
                },
            )
            .await?
        }
        Commands::DownloadBars {
            from,
            to,
            timeframe,
            symbols,
            feed,
            limit,
            output,
        } => {
            data_download::run(
                cfg,
                DownloadBarsOptions {
                    from,
                    to,
                    timeframe,
                    symbols,
                    feed,
                    limit,
                    output,
                },
            )
            .await?
        }
    }
    Ok(())
}

enum GatewayMode {
    Mock,
    Alpaca,
}

async fn run_engine(cfg: AppConfig, mode: GatewayMode) -> Result<()> {
    let store = AuditStore::new(&cfg.storage).await?;
    store.migrate().await?;

    let pair_engine = PairEngine::new(&cfg);
    let risk = RiskEngine::new(cfg.risk.clone());

    match mode {
        GatewayMode::Mock => {
            let gateway = MockGateway::new(cfg.symbol_universe());
            run_loop(cfg, gateway, None, pair_engine, risk, store).await?;
        }
        GatewayMode::Alpaca => {
            let gateway = AlpacaClient::from_config(&cfg.alpaca)?;
            run_loop(
                cfg,
                gateway.clone(),
                Some(gateway),
                pair_engine,
                risk,
                store,
            )
            .await?;
        }
    }
    Ok(())
}

async fn run_loop<G>(
    cfg: AppConfig,
    gateway: G,
    bar_client: Option<AlpacaClient>,
    mut pair_engine: PairEngine,
    mut risk: RiskEngine,
    store: AuditStore,
) -> Result<()>
where
    G: BrokerGateway + Clone + Send + Sync + 'static,
{
    info!("starting ETF pairs engine in {} mode", cfg.engine.mode);
    let mut executor = PairExecutor::new(gateway.clone(), cfg.execution.clone(), cfg.risk.clone());
    for pair_id in executor.reconcile_positions(&cfg.pairs, &store).await? {
        risk.mark_open(&pair_id);
    }
    let signal_timeframes = cfg.signal_timeframes();
    let use_alpaca_bars = bar_client.is_some()
        && cfg
            .engine
            .signal_bar_source
            .as_deref()
            .unwrap_or("alpaca")
            .eq_ignore_ascii_case("alpaca");
    let mut bar_poller = bar_client.map(|client| {
        AlpacaBarPoller::new(
            client,
            cfg.symbol_universe(),
            signal_timeframes.clone(),
            cfg.engine
                .signal_bar_feed
                .clone()
                .unwrap_or_else(|| "iex".to_string()),
            cfg.engine.signal_bar_poll_seconds.unwrap_or(60),
        )
    });
    let mut bar_aggregator = BarAggregator::new(signal_timeframes.clone());
    if !signal_timeframes.is_empty() && !use_alpaca_bars {
        match store.load_bar_states().await {
            Ok(states) if !states.is_empty() => {
                let count = states.len();
                bar_aggregator.restore(states);
                info!(count, "restored persisted bar aggregator state");
            }
            Ok(_) => {}
            Err(e) => warn!(error=%e, "failed to restore bar aggregator state"),
        }
        info!(
            timeframes = %signal_timeframes
                .iter()
                .map(|timeframe| timeframe.as_str())
                .collect::<Vec<_>>()
                .join(","),
            "bar aggregation enabled for signal generation"
        );
    } else if !signal_timeframes.is_empty() {
        info!(
            timeframes = %signal_timeframes
                .iter()
                .map(|timeframe| timeframe.as_str())
                .collect::<Vec<_>>()
                .join(","),
            feed = %cfg.engine.signal_bar_feed.as_deref().unwrap_or("iex"),
            "Alpaca completed-bar polling enabled for signal generation"
        );
    }
    let mut tick = tokio::time::interval(Duration::from_millis(cfg.engine.loop_ms));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tick.tick().await;
        let started = std::time::Instant::now();

        let quotes = gateway.latest_quotes(&cfg.symbol_universe()).await?;
        risk.update_equity(
            gateway
                .account_equity()
                .await
                .unwrap_or(cfg.risk.initial_equity),
        );

        let mut decisions = Vec::new();
        if pair_engine.has_tick_pairs() {
            decisions.extend(pair_engine.on_quotes(&quotes, &risk).await);
        }
        if use_alpaca_bars {
            if let Some(poller) = &mut bar_poller {
                match poller.poll(chrono::Utc::now()).await {
                    Ok(completed_bars) => {
                        for (timeframe, bar_quotes) in completed_bars {
                            decisions.extend(
                                pair_engine
                                    .on_quotes_for_timeframe(&bar_quotes, &risk, timeframe)
                                    .await,
                            );
                        }
                    }
                    Err(e) => warn!(error=%e, "failed to poll Alpaca signal bars"),
                }
            }
        } else {
            let completed_bars = bar_aggregator.update(&quotes);
            if !signal_timeframes.is_empty() {
                if let Err(e) = store.save_bar_states(&bar_aggregator.snapshot()).await {
                    warn!(error=%e, "failed to persist bar aggregator state");
                }
            }
            for (timeframe, bar_quotes) in completed_bars {
                decisions.extend(
                    pair_engine
                        .on_quotes_for_timeframe(&bar_quotes, &risk, timeframe)
                        .await,
                );
            }
        }
        for decision in decisions {
            store.record_signal(&decision).await.ok();
            if !risk.allow_decision(&decision) {
                warn!(pair=%decision.pair_id, ?decision.action, "risk blocked decision");
                continue;
            }
            match executor.handle_decision(&decision, &quotes, &store).await {
                Ok(ExecutionOutcome::Opened(pair_id)) => risk.mark_open(&pair_id),
                Ok(ExecutionOutcome::Closed(pair_id)) => risk.mark_closed(&pair_id),
                Ok(ExecutionOutcome::None) => {}
                Err(e) => {
                    error!(pair=%decision.pair_id, error=%e, "execution error");
                    store
                        .record_risk_event("execution_error", &decision.pair_id, &e.to_string())
                        .await
                        .ok();
                }
            }
        }

        let elapsed = started.elapsed();
        if elapsed.as_millis() > cfg.engine.warn_loop_ms as u128 {
            warn!(?elapsed, "slow loop iteration");
        }
    }
}

async fn check_assets(cfg: AppConfig) -> Result<()> {
    let client = AlpacaClient::from_config(&cfg.alpaca)?;
    for symbol in cfg.symbol_universe() {
        let asset = client.asset(&symbol).await?;
        println!(
            "{symbol}: tradable={} shortable={} easy_to_borrow={} marginable={} status={}",
            asset.tradable, asset.shortable, asset.easy_to_borrow, asset.marginable, asset.status
        );
    }
    Ok(())
}
