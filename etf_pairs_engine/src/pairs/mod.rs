use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

use crate::alpaca::Quote;
use crate::config::{AppConfig, PairConfig, SignalTimeframe, StrategyConfig};
use crate::risk::RiskEngine;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecisionAction {
    EnterLongSpread,
    EnterShortSpread,
    Exit,
    Hold,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairDecision {
    pub ts: DateTime<Utc>,
    pub pair_id: String,
    pub a: String,
    pub b: String,
    pub beta: f64,
    pub spread: f64,
    pub mean: f64,
    pub std: f64,
    pub z_score: f64,
    pub correlation: f64,
    pub action: DecisionAction,
    pub reason: String,
    pub leg_notional: f64,
    pub max_holding_bars: Option<u64>,
    pub adverse_zscore: Option<f64>,
    pub stop_loss_dollars: Option<f64>,
    pub profit_protection_min_dollars: Option<f64>,
    pub profit_protection_retrace_fraction: Option<f64>,
    pub profit_protection_floor_dollars: Option<f64>,
}

#[derive(Debug, Clone)]
struct PairState {
    cfg: PairConfig,
    strategy: PairRuntimeConfig,
    spreads: VecDeque<f64>,
    a_prices: VecDeque<f64>,
    b_prices: VecDeque<f64>,
    a_returns: VecDeque<f64>,
    b_returns: VecDeque<f64>,
    last_a_mid: Option<f64>,
    last_b_mid: Option<f64>,
    open_bars: Option<u64>,
    open_entry_z: Option<f64>,
    pending_entry: Option<PendingEntry>,
}

#[derive(Debug, Clone)]
struct PairRuntimeConfig {
    rolling_window_ticks: usize,
    entry_zscore: f64,
    exit_zscore: f64,
    min_samples: usize,
    max_spread_bps: f64,
    use_rolling_beta: bool,
    beta_min: Option<f64>,
    beta_max: Option<f64>,
    min_correlation: Option<f64>,
    max_spread_std_bps: Option<f64>,
    entry_confirmation_bars: u64,
    min_expected_spread_move_after_costs: Option<f64>,
    adverse_zscore: Option<f64>,
    stop_loss_dollars: Option<f64>,
    profit_protection_min_dollars: Option<f64>,
    profit_protection_retrace_fraction: Option<f64>,
    profit_protection_floor_dollars: Option<f64>,
    max_holding_bars: Option<u64>,
    signal_timeframe: SignalTimeframe,
}

#[derive(Debug, Clone, Copy)]
struct PendingEntry {
    action: DecisionAction,
    extreme_z: f64,
    confirmations: u64,
}

impl PairRuntimeConfig {
    fn resolve(global: &StrategyConfig, pair: &PairConfig) -> Self {
        let rolling_window_ticks = pair
            .rolling_window_ticks
            .unwrap_or(global.rolling_window_ticks);
        Self {
            rolling_window_ticks,
            entry_zscore: pair.entry_zscore.unwrap_or(global.entry_zscore),
            exit_zscore: pair.exit_zscore.unwrap_or(global.exit_zscore),
            min_samples: pair.min_samples.unwrap_or(global.min_samples),
            max_spread_bps: pair.max_spread_bps.unwrap_or(global.max_spread_bps),
            use_rolling_beta: pair
                .use_rolling_beta
                .or(global.use_rolling_beta)
                .unwrap_or(false),
            beta_min: pair.beta_min.or(global.beta_min),
            beta_max: pair.beta_max.or(global.beta_max),
            min_correlation: pair.min_correlation.or(global.min_correlation),
            max_spread_std_bps: pair.max_spread_std_bps.or(global.max_spread_std_bps),
            entry_confirmation_bars: pair
                .entry_confirmation_bars
                .or(global.entry_confirmation_bars)
                .unwrap_or(0),
            min_expected_spread_move_after_costs: pair
                .min_expected_spread_move_after_costs
                .or(global.min_expected_spread_move_after_costs),
            adverse_zscore: pair.adverse_zscore.or(global.adverse_zscore),
            stop_loss_dollars: pair.stop_loss_dollars.or(global.stop_loss_dollars),
            profit_protection_min_dollars: pair
                .profit_protection_min_dollars
                .or(global.profit_protection_min_dollars),
            profit_protection_retrace_fraction: pair
                .profit_protection_retrace_fraction
                .or(global.profit_protection_retrace_fraction),
            profit_protection_floor_dollars: pair
                .profit_protection_floor_dollars
                .or(global.profit_protection_floor_dollars),
            max_holding_bars: pair.max_holding_bars,
            signal_timeframe: pair.signal_timeframe.unwrap_or(SignalTimeframe::Tick),
        }
    }
}

pub struct PairEngine {
    pairs: Vec<PairState>,
}

impl PairEngine {
    pub fn new(cfg: &AppConfig) -> Self {
        let global_strategy = cfg.strategy.clone();
        let pairs = cfg
            .pairs
            .iter()
            .filter(|p| p.enabled)
            .cloned()
            .map(|pair_cfg| PairState {
                strategy: PairRuntimeConfig::resolve(&global_strategy, &pair_cfg),
                cfg: pair_cfg,
                spreads: VecDeque::new(),
                a_prices: VecDeque::new(),
                b_prices: VecDeque::new(),
                a_returns: VecDeque::new(),
                b_returns: VecDeque::new(),
                last_a_mid: None,
                last_b_mid: None,
                open_bars: None,
                open_entry_z: None,
                pending_entry: None,
            })
            .collect();
        Self { pairs }
    }

    pub async fn on_quotes(
        &mut self,
        quotes: &HashMap<String, Quote>,
        risk: &RiskEngine,
    ) -> Vec<PairDecision> {
        self.on_quotes_for_timeframe(quotes, risk, SignalTimeframe::Tick)
            .await
    }

    pub async fn on_quotes_for_timeframe(
        &mut self,
        quotes: &HashMap<String, Quote>,
        risk: &RiskEngine,
        timeframe: SignalTimeframe,
    ) -> Vec<PairDecision> {
        let mut out = Vec::with_capacity(self.pairs.len());
        for p in &mut self.pairs {
            if p.strategy.signal_timeframe != timeframe {
                continue;
            }
            let Some(qa) = quotes.get(&p.cfg.a) else {
                continue;
            };
            let Some(qb) = quotes.get(&p.cfg.b) else {
                continue;
            };
            if qa.spread_bps() > p.strategy.max_spread_bps
                || qb.spread_bps() > p.strategy.max_spread_bps
            {
                out.push(Self::decision(
                    p,
                    qa.ts,
                    p.cfg.beta.unwrap_or(1.0),
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    DecisionAction::Blocked,
                    "quote spread too wide",
                ));
                continue;
            }
            let a_mid = qa.mid();
            let b_mid = qb.mid();
            push_cap(&mut p.a_prices, a_mid, p.strategy.rolling_window_ticks);
            push_cap(&mut p.b_prices, b_mid, p.strategy.rolling_window_ticks);
            let beta = if p.strategy.use_rolling_beta && p.a_prices.len() >= p.strategy.min_samples
            {
                rolling_beta(&p.a_prices, &p.b_prices)
                    .unwrap_or_else(|| p.cfg.beta.unwrap_or(1.0))
                    .clamp(
                        p.strategy.beta_min.unwrap_or(f64::NEG_INFINITY),
                        p.strategy.beta_max.unwrap_or(f64::INFINITY),
                    )
            } else {
                p.cfg.beta.unwrap_or(1.0)
            };
            let spread = a_mid - beta * b_mid;
            push_cap(&mut p.spreads, spread, p.strategy.rolling_window_ticks);
            if let (Some(la), Some(lb)) = (p.last_a_mid, p.last_b_mid) {
                push_cap(
                    &mut p.a_returns,
                    (a_mid / la).ln(),
                    p.strategy.rolling_window_ticks,
                );
                push_cap(
                    &mut p.b_returns,
                    (b_mid / lb).ln(),
                    p.strategy.rolling_window_ticks,
                );
            }
            p.last_a_mid = Some(a_mid);
            p.last_b_mid = Some(b_mid);

            if p.spreads.len() < p.strategy.min_samples {
                out.push(Self::decision(
                    p,
                    qa.ts,
                    beta,
                    spread,
                    0.0,
                    0.0,
                    0.0,
                    DecisionAction::Hold,
                    "warming up rolling window",
                ));
                continue;
            }
            let mean = mean(&p.spreads);
            let std = stddev(&p.spreads, mean).max(1e-9);
            let z = (spread - mean) / std;
            let corr = correlation(&p.a_returns, &p.b_returns).unwrap_or(0.0);
            if p.strategy
                .min_correlation
                .map(|threshold| corr < threshold)
                .unwrap_or(false)
            {
                p.pending_entry = None;
                out.push(Self::decision(
                    p,
                    qa.ts,
                    beta,
                    spread,
                    mean,
                    std,
                    corr,
                    DecisionAction::Hold,
                    "rolling correlation below minimum",
                ));
                continue;
            }
            let spread_std_bps = std / a_mid.max(1e-9) * 10_000.0;
            if p.strategy
                .max_spread_std_bps
                .map(|max_std| spread_std_bps > max_std)
                .unwrap_or(false)
            {
                p.pending_entry = None;
                out.push(Self::decision(
                    p,
                    qa.ts,
                    beta,
                    spread,
                    mean,
                    std,
                    corr,
                    DecisionAction::Hold,
                    "spread volatility above maximum",
                ));
                continue;
            }
            if risk.has_open_pair(&p.cfg.id) {
                let holding_bars = p.open_bars.unwrap_or(0).saturating_add(1);
                p.open_bars = Some(holding_bars);
                let exit = z.abs() <= p.strategy.exit_zscore;
                let adverse_z_exit = p
                    .strategy
                    .adverse_zscore
                    .map(|threshold| {
                        let entry_z = p.open_entry_z.unwrap_or(z);
                        let entry_side = entry_z.signum();
                        entry_side.abs() > 0.0 && entry_side * (z - entry_z) >= threshold
                    })
                    .unwrap_or(false);
                let max_holding_exit = p
                    .strategy
                    .max_holding_bars
                    .map(|max_holding_bars| holding_bars >= max_holding_bars)
                    .unwrap_or(false);
                let action = if exit || adverse_z_exit || max_holding_exit {
                    DecisionAction::Exit
                } else {
                    DecisionAction::Hold
                };
                out.push(Self::decision(
                    p,
                    qa.ts,
                    beta,
                    spread,
                    mean,
                    std,
                    corr,
                    action,
                    if max_holding_exit {
                        "max holding bars reached"
                    } else if adverse_z_exit {
                        "adverse z-score stop"
                    } else if exit {
                        "spread converged"
                    } else {
                        "position open"
                    },
                ));
                continue;
            }
            p.open_bars = None;
            p.open_entry_z = None;
            let expected_move = ((z.abs() - p.strategy.exit_zscore).max(0.0) * std)
                * (p.cfg.leg_notional / a_mid.max(1e-9));
            if p.strategy
                .min_expected_spread_move_after_costs
                .map(|min_move| expected_move < min_move)
                .unwrap_or(false)
            {
                out.push(Self::decision(
                    p,
                    qa.ts,
                    beta,
                    spread,
                    mean,
                    std,
                    corr,
                    DecisionAction::Hold,
                    "expected spread move below minimum",
                ));
                continue;
            }
            let action = if z >= p.strategy.entry_zscore {
                DecisionAction::EnterShortSpread
            } else if z <= -p.strategy.entry_zscore {
                DecisionAction::EnterLongSpread
            } else {
                DecisionAction::Hold
            };
            let action = Self::confirm_entry(p, action, z);
            if matches!(
                action,
                DecisionAction::EnterLongSpread | DecisionAction::EnterShortSpread
            ) {
                p.open_entry_z = Some(z);
            }
            let reason = match action {
                DecisionAction::EnterShortSpread => "spread high: short A / long B",
                DecisionAction::EnterLongSpread => "spread low: long A / short B",
                _ => "no entry signal",
            };
            out.push(Self::decision(
                p, qa.ts, beta, spread, mean, std, corr, action, reason,
            ));
        }
        out
    }

    fn confirm_entry(p: &mut PairState, action: DecisionAction, z: f64) -> DecisionAction {
        if p.strategy.entry_confirmation_bars == 0 {
            p.pending_entry = None;
            return action;
        }
        if !matches!(
            action,
            DecisionAction::EnterLongSpread | DecisionAction::EnterShortSpread
        ) {
            p.pending_entry = None;
            return DecisionAction::Hold;
        }

        match p.pending_entry {
            Some(mut pending) if pending.action == action => {
                if z.abs() < pending.extreme_z.abs() {
                    pending.confirmations += 1;
                } else {
                    pending.extreme_z = z;
                    pending.confirmations = 0;
                }
                if pending.confirmations >= p.strategy.entry_confirmation_bars {
                    p.pending_entry = None;
                    action
                } else {
                    p.pending_entry = Some(pending);
                    DecisionAction::Hold
                }
            }
            _ => {
                p.pending_entry = Some(PendingEntry {
                    action,
                    extreme_z: z,
                    confirmations: 0,
                });
                DecisionAction::Hold
            }
        }
    }

    pub fn has_tick_pairs(&self) -> bool {
        self.pairs
            .iter()
            .any(|p| p.strategy.signal_timeframe == SignalTimeframe::Tick)
    }

    #[allow(clippy::too_many_arguments)]
    fn decision(
        p: &PairState,
        ts: DateTime<Utc>,
        beta: f64,
        spread: f64,
        mean: f64,
        std: f64,
        corr: f64,
        action: DecisionAction,
        reason: &str,
    ) -> PairDecision {
        let z = if std.abs() < 1e-12 {
            0.0
        } else {
            (spread - mean) / std
        };
        PairDecision {
            ts,
            pair_id: p.cfg.id.clone(),
            a: p.cfg.a.clone(),
            b: p.cfg.b.clone(),
            beta,
            spread,
            mean,
            std,
            z_score: z,
            correlation: corr,
            action,
            reason: reason.into(),
            leg_notional: p.cfg.leg_notional,
            max_holding_bars: p.strategy.max_holding_bars,
            adverse_zscore: p.strategy.adverse_zscore,
            stop_loss_dollars: p.strategy.stop_loss_dollars,
            profit_protection_min_dollars: p.strategy.profit_protection_min_dollars,
            profit_protection_retrace_fraction: p.strategy.profit_protection_retrace_fraction,
            profit_protection_floor_dollars: p.strategy.profit_protection_floor_dollars,
        }
    }
}

fn push_cap(q: &mut VecDeque<f64>, v: f64, cap: usize) {
    q.push_back(v);
    while q.len() > cap {
        q.pop_front();
    }
}
fn mean(q: &VecDeque<f64>) -> f64 {
    q.iter().sum::<f64>() / q.len() as f64
}
fn stddev(q: &VecDeque<f64>, m: f64) -> f64 {
    (q.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (q.len().saturating_sub(1).max(1)) as f64)
        .sqrt()
}

fn rolling_beta(a: &VecDeque<f64>, b: &VecDeque<f64>) -> Option<f64> {
    let n = a.len().min(b.len());
    if n < 3 {
        return None;
    }
    let aa: Vec<_> = a.iter().rev().take(n).copied().collect();
    let bb: Vec<_> = b.iter().rev().take(n).copied().collect();
    let ma = aa.iter().sum::<f64>() / n as f64;
    let mb = bb.iter().sum::<f64>() / n as f64;
    let mut cov = 0.0;
    let mut vb = 0.0;
    for i in 0..n {
        cov += (aa[i] - ma) * (bb[i] - mb);
        vb += (bb[i] - mb).powi(2);
    }
    if vb.abs() < 1e-12 {
        None
    } else {
        Some(cov / vb)
    }
}

fn correlation(a: &VecDeque<f64>, b: &VecDeque<f64>) -> Option<f64> {
    let n = a.len().min(b.len());
    if n < 3 {
        return None;
    }
    let aa: Vec<_> = a.iter().rev().take(n).copied().collect();
    let bb: Vec<_> = b.iter().rev().take(n).copied().collect();
    let ma = aa.iter().sum::<f64>() / n as f64;
    let mb = bb.iter().sum::<f64>() / n as f64;
    let mut cov = 0.0;
    let mut va = 0.0;
    let mut vb = 0.0;
    for i in 0..n {
        cov += (aa[i] - ma) * (bb[i] - mb);
        va += (aa[i] - ma).powi(2);
        vb += (bb[i] - mb).powi(2);
    }
    Some(cov / (va.sqrt() * vb.sqrt()).max(1e-12))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn corr_positive() {
        let a = VecDeque::from(vec![1., 2., 3.]);
        let b = VecDeque::from(vec![2., 4., 6.]);
        assert!(correlation(&a, &b).unwrap() > 0.99);
    }

    #[test]
    fn beta_estimates_linear_relationship() {
        let b = VecDeque::from(vec![10., 11., 12., 13., 14.]);
        let a = VecDeque::from(vec![20., 22., 24., 26., 28.]);
        assert!((rolling_beta(&a, &b).unwrap() - 2.0).abs() < 1e-9);
    }
}
