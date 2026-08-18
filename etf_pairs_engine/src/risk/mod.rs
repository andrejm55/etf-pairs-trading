use crate::config::RiskConfig;
use crate::pairs::{DecisionAction, PairDecision};
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct RiskEngine {
    cfg: RiskConfig,
    equity: f64,
    start_equity: f64,
    open_pairs: HashSet<String>,
}
impl RiskEngine {
    pub fn new(cfg: RiskConfig) -> Self {
        Self {
            start_equity: cfg.initial_equity,
            equity: cfg.initial_equity,
            open_pairs: HashSet::new(),
            cfg,
        }
    }
    pub fn update_equity(&mut self, equity: f64) {
        self.equity = equity;
    }
    pub fn has_open_pair(&self, pair: &str) -> bool {
        self.open_pairs.contains(pair)
    }
    pub fn allow_decision(&self, d: &PairDecision) -> bool {
        if self.start_equity - self.equity > self.cfg.max_daily_loss {
            return false;
        }
        match d.action {
            DecisionAction::EnterLongSpread | DecisionAction::EnterShortSpread => {
                let pair_notional = d.leg_notional * 2.0;
                let projected_total_notional = (self.open_pairs.len() as f64 + 1.0) * pair_notional;
                self.open_pairs.len() < self.cfg.max_open_pairs
                    && pair_notional <= self.cfg.max_pair_notional
                    && projected_total_notional <= self.cfg.max_total_notional
            }
            _ => true,
        }
    }
    pub fn mark_open(&mut self, pair: &str) {
        self.open_pairs.insert(pair.to_string());
    }
    pub fn mark_closed(&mut self, pair: &str) {
        self.open_pairs.remove(pair);
    }
}
