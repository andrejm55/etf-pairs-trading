use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use uuid::Uuid;

use crate::alpaca::{BrokerGateway, NewOrder, OrderStatus, Position, Quote, Side};
use crate::config::{ExecutionConfig, PairConfig, RiskConfig};
use crate::pairs::{DecisionAction, PairDecision};
use crate::storage::AuditStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegIntent {
    BuyA,
    SellA,
    BuyB,
    SellB,
}

#[derive(Debug, Clone)]
struct TrackedLeg {
    symbol: String,
    qty: f64,
    side: Side,
    entry_price: Option<f64>,
}

#[derive(Debug, Clone)]
struct TrackedPair {
    a: TrackedLeg,
    b: TrackedLeg,
    max_open_pnl: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionOutcome {
    None,
    Opened(String),
    Closed(String),
}

pub struct PairExecutor<G: BrokerGateway + Clone> {
    gateway: G,
    cfg: ExecutionConfig,
    risk_cfg: RiskConfig,
    active_pairs: HashMap<String, TrackedPair>,
}

impl<G: BrokerGateway + Clone> PairExecutor<G> {
    pub fn new(gateway: G, cfg: ExecutionConfig, risk_cfg: RiskConfig) -> Self {
        Self {
            gateway,
            cfg,
            risk_cfg,
            active_pairs: HashMap::new(),
        }
    }

    pub async fn reconcile_positions(
        &mut self,
        pairs: &[PairConfig],
        store: &AuditStore,
    ) -> Result<Vec<String>> {
        let positions = self.gateway.open_positions().await?;
        let mut reconciled = Vec::new();
        for pair in pairs.iter().filter(|p| p.enabled) {
            let qty_a = position_qty(&positions, &pair.a);
            let qty_b = position_qty(&positions, &pair.b);
            if qty_a.abs() <= f64::EPSILON || qty_b.abs() <= f64::EPSILON {
                continue;
            }
            let Some((side_a, side_b)) = sides_from_position(qty_a, qty_b) else {
                store
                    .record_risk_event(
                        "reconcile_ambiguous",
                        &pair.id,
                        "both pair legs point the same direction; manual review required",
                    )
                    .await
                    .ok();
                continue;
            };
            self.active_pairs.insert(
                pair.id.clone(),
                TrackedPair {
                    a: TrackedLeg {
                        symbol: pair.a.clone(),
                        qty: qty_a.abs(),
                        side: side_a,
                        entry_price: None,
                    },
                    b: TrackedLeg {
                        symbol: pair.b.clone(),
                        qty: qty_b.abs(),
                        side: side_b,
                        entry_price: None,
                    },
                    max_open_pnl: 0.0,
                },
            );
            store
                .record_execution(
                    &pair.id,
                    "reconciled_position",
                    &format!("{} {} / {} {}", pair.a, qty_a, pair.b, qty_b),
                )
                .await
                .ok();
            reconciled.push(pair.id.clone());
        }
        Ok(reconciled)
    }

    pub async fn handle_decision(
        &mut self,
        d: &PairDecision,
        quotes: &HashMap<String, Quote>,
        store: &AuditStore,
    ) -> Result<ExecutionOutcome> {
        match d.action {
            DecisionAction::EnterLongSpread => {
                self.enter(d, quotes, LegIntent::BuyA, LegIntent::SellB, store)
                    .await
            }
            DecisionAction::EnterShortSpread => {
                self.enter(d, quotes, LegIntent::SellA, LegIntent::BuyB, store)
                    .await
            }
            DecisionAction::Exit => self.exit(d, quotes, store).await,
            _ => {
                if self.should_profit_protect(d, quotes) {
                    store
                        .record_execution(
                            &d.pair_id,
                            "profit_protection_exit",
                            "open pair gave back protected unrealized profit",
                        )
                        .await
                        .ok();
                    self.exit(d, quotes, store).await
                } else {
                    Ok(ExecutionOutcome::None)
                }
            }
        }
    }

    async fn enter(
        &mut self,
        d: &PairDecision,
        quotes: &HashMap<String, Quote>,
        leg1: LegIntent,
        leg2: LegIntent,
        store: &AuditStore,
    ) -> Result<ExecutionOutcome> {
        if self.active_pairs.contains_key(&d.pair_id) {
            return Ok(ExecutionOutcome::None);
        }

        let o1 = self.order_from_leg(d, quotes, leg1, 1.0, self.cfg.passive_entry)?;
        let o2 = self.order_from_leg(d, quotes, leg2, 1.0, self.cfg.passive_entry)?;
        let q1 = quotes
            .get(&o1.symbol)
            .ok_or_else(|| anyhow!("missing quote for {}", o1.symbol))?;
        let q2 = quotes
            .get(&o2.symbol)
            .ok_or_else(|| anyhow!("missing quote for {}", o2.symbol))?;
        self.check_entry_shortability(&o1).await?;
        self.check_entry_shortability(&o2).await?;

        let (qty1, qty2) = balanced_quantities(d.leg_notional, q1.mid(), q2.mid());
        let gross_notional = qty1 * q1.mid() + qty2 * q2.mid();
        if gross_notional > self.risk_cfg.max_pair_notional {
            return Err(anyhow!(
                "balanced pair gross notional {:.2} exceeds max_pair_notional {:.2}",
                gross_notional,
                self.risk_cfg.max_pair_notional
            ));
        }

        let o1 = NewOrder { qty: qty1, ..o1 };
        let o2 = NewOrder { qty: qty2, ..o2 };
        let ack1 = self
            .submit_and_record(d, &o1, "submitted_entry_leg1", store)
            .await?;
        let ack2 = self
            .submit_and_record(d, &o2, "submitted_entry_leg2", store)
            .await?;
        let (s1, s2) = self.wait_pair_orders(&ack1.id, &ack2.id).await?;

        if s1.is_filled() && s2.is_filled() {
            self.active_pairs.insert(
                d.pair_id.clone(),
                TrackedPair {
                    a: tracked_leg_for(&o1, &d.a, &d.b)?,
                    b: tracked_leg_for(&o2, &d.a, &d.b)?,
                    max_open_pnl: 0.0,
                },
            );
            store
                .record_execution(
                    &d.pair_id,
                    "pair_entry_filled",
                    &format!(
                        "{} + {}; approx notionals {:.2} / {:.2}",
                        ack1.id,
                        ack2.id,
                        o1.qty * q1.mid(),
                        o2.qty * q2.mid()
                    ),
                )
                .await
                .ok();
            return Ok(ExecutionOutcome::Opened(d.pair_id.clone()));
        }

        self.cancel_if_open(&s1).await;
        self.cancel_if_open(&s2).await;
        self.flatten_filled_leg(d, &o1, &s1, quotes, store)
            .await
            .ok();
        self.flatten_filled_leg(d, &o2, &s2, quotes, store)
            .await
            .ok();
        Err(anyhow!(
            "entry did not complete: {} filled {}/{} status {}; {} filled {}/{} status {}",
            s1.symbol,
            s1.filled_qty,
            s1.qty,
            s1.status,
            s2.symbol,
            s2.filled_qty,
            s2.qty,
            s2.status
        ))
    }

    async fn exit(
        &mut self,
        d: &PairDecision,
        quotes: &HashMap<String, Quote>,
        store: &AuditStore,
    ) -> Result<ExecutionOutcome> {
        let Some(position) = self.active_pairs.get(&d.pair_id).cloned() else {
            store
                .record_execution(&d.pair_id, "exit_ignored", "no tracked open pair")
                .await
                .ok();
            return Ok(ExecutionOutcome::None);
        };

        let o1 = self.order_from_tracked_leg(d, quotes, &position.a, true)?;
        let o2 = self.order_from_tracked_leg(d, quotes, &position.b, true)?;
        let ack1 = self
            .submit_and_record(d, &o1, "submitted_exit_leg1", store)
            .await?;
        let ack2 = self
            .submit_and_record(d, &o2, "submitted_exit_leg2", store)
            .await?;
        let (s1, s2) = self.wait_pair_orders(&ack1.id, &ack2.id).await?;

        if s1.is_filled() && s2.is_filled() {
            self.active_pairs.remove(&d.pair_id);
            store
                .record_execution(
                    &d.pair_id,
                    "pair_exit_filled",
                    &format!("{} + {}", ack1.id, ack2.id),
                )
                .await
                .ok();
            return Ok(ExecutionOutcome::Closed(d.pair_id.clone()));
        }

        self.cancel_if_open(&s1).await;
        self.cancel_if_open(&s2).await;
        if self.cfg.use_aggressive_rescue {
            self.flatten_remaining_leg(d, &o1, &s1, quotes, store)
                .await
                .ok();
            self.flatten_remaining_leg(d, &o2, &s2, quotes, store)
                .await
                .ok();
        }
        Err(anyhow!(
            "exit did not complete: {} filled {}/{} status {}; {} filled {}/{} status {}",
            s1.symbol,
            s1.filled_qty,
            s1.qty,
            s1.status,
            s2.symbol,
            s2.filled_qty,
            s2.qty,
            s2.status
        ))
    }

    async fn submit_and_record(
        &self,
        d: &PairDecision,
        order: &NewOrder,
        status: &str,
        store: &AuditStore,
    ) -> Result<crate::alpaca::OrderAck> {
        let ack = self.gateway.submit_limit_order(order.clone()).await?;
        store
            .record_order(
                &d.pair_id,
                &ack.id,
                &order.symbol,
                &format!("{:?}", order.side),
                order.qty,
                order.limit_price,
                status,
            )
            .await
            .ok();
        Ok(ack)
    }

    async fn wait_pair_orders(
        &self,
        order_1: &str,
        order_2: &str,
    ) -> Result<(OrderStatus, OrderStatus)> {
        let timeout_secs = self
            .cfg
            .max_leg_delay_seconds
            .min(self.cfg.stale_order_seconds)
            .max(1);
        let deadline = Instant::now() + Duration::from_secs(timeout_secs);
        let mut s1 = self.gateway.order_status(order_1).await?;
        let mut s2 = self.gateway.order_status(order_2).await?;

        while Instant::now() < deadline {
            if (s1.is_filled() || s1.is_terminal()) && (s2.is_filled() || s2.is_terminal()) {
                return Ok((s1, s2));
            }
            sleep(Duration::from_millis(250)).await;
            s1 = self.gateway.order_status(order_1).await?;
            s2 = self.gateway.order_status(order_2).await?;
        }
        Ok((s1, s2))
    }

    async fn cancel_if_open(&self, status: &OrderStatus) {
        if !status.is_terminal() {
            self.gateway.cancel_order(&status.id).await.ok();
        }
    }

    async fn check_entry_shortability(&self, order: &NewOrder) -> Result<()> {
        if !matches!(order.side, Side::Sell) {
            return Ok(());
        }
        let asset = self.gateway.asset(&order.symbol).await?;
        if !asset.tradable || !asset.shortable {
            return Err(anyhow!(
                "{} is not tradable/shortable; refusing ETF short leg",
                order.symbol
            ));
        }
        if self.risk_cfg.require_easy_to_borrow_for_short && !asset.easy_to_borrow {
            return Err(anyhow!(
                "{} is not easy-to-borrow; refusing ETF short leg",
                order.symbol
            ));
        }
        Ok(())
    }

    async fn flatten_filled_leg(
        &self,
        d: &PairDecision,
        order: &NewOrder,
        status: &OrderStatus,
        quotes: &HashMap<String, Quote>,
        store: &AuditStore,
    ) -> Result<()> {
        if status.filled_qty <= f64::EPSILON {
            return Ok(());
        }
        let rescue = self.rescue_order(
            d,
            quotes,
            order.symbol.clone(),
            order.side.opposite(),
            status.filled_qty,
        )?;
        self.submit_and_record(d, &rescue, "emergency_flatten_filled_leg", store)
            .await?;
        Ok(())
    }

    async fn flatten_remaining_leg(
        &self,
        d: &PairDecision,
        order: &NewOrder,
        status: &OrderStatus,
        quotes: &HashMap<String, Quote>,
        store: &AuditStore,
    ) -> Result<()> {
        let remaining_qty = (status.qty - status.filled_qty).max(0.0);
        if remaining_qty <= f64::EPSILON {
            return Ok(());
        }
        let rescue =
            self.rescue_order(d, quotes, order.symbol.clone(), order.side, remaining_qty)?;
        self.submit_and_record(d, &rescue, "emergency_flatten_remaining_leg", store)
            .await?;
        Ok(())
    }

    fn rescue_order(
        &self,
        d: &PairDecision,
        quotes: &HashMap<String, Quote>,
        symbol: String,
        side: Side,
        qty: f64,
    ) -> Result<NewOrder> {
        let q = quotes
            .get(&symbol)
            .ok_or_else(|| anyhow!("missing quote for {symbol}"))?;
        Ok(NewOrder {
            symbol,
            qty,
            side,
            limit_price: aggressive_price(q, side),
            client_order_id: format!(
                "{}-{}-rescue-{}",
                self.cfg.client_order_prefix,
                d.pair_id,
                Uuid::new_v4()
            ),
        })
    }

    fn order_from_leg(
        &self,
        d: &PairDecision,
        quotes: &HashMap<String, Quote>,
        intent: LegIntent,
        qty: f64,
        passive: bool,
    ) -> Result<NewOrder> {
        let (symbol, side) = match intent {
            LegIntent::BuyA => (&d.a, Side::Buy),
            LegIntent::SellA => (&d.a, Side::Sell),
            LegIntent::BuyB => (&d.b, Side::Buy),
            LegIntent::SellB => (&d.b, Side::Sell),
        };
        self.order_for_symbol(d, quotes, symbol, side, qty, passive)
    }

    fn order_from_tracked_leg(
        &self,
        d: &PairDecision,
        quotes: &HashMap<String, Quote>,
        leg: &TrackedLeg,
        aggressive: bool,
    ) -> Result<NewOrder> {
        self.order_for_symbol(
            d,
            quotes,
            &leg.symbol,
            leg.side.opposite(),
            leg.qty,
            !aggressive,
        )
    }

    fn order_for_symbol(
        &self,
        d: &PairDecision,
        quotes: &HashMap<String, Quote>,
        symbol: &str,
        side: Side,
        qty: f64,
        passive: bool,
    ) -> Result<NewOrder> {
        let q = quotes
            .get(symbol)
            .ok_or_else(|| anyhow!("missing quote for {symbol}"))?;
        let limit_price = if passive {
            passive_price(q, side)
        } else {
            aggressive_price(q, side)
        };
        Ok(NewOrder {
            symbol: symbol.to_string(),
            qty,
            side,
            limit_price,
            client_order_id: format!(
                "{}-{}-{}",
                self.cfg.client_order_prefix,
                d.pair_id,
                Uuid::new_v4()
            ),
        })
    }

    fn should_profit_protect(&mut self, d: &PairDecision, quotes: &HashMap<String, Quote>) -> bool {
        let Some(min_profit) = d.profit_protection_min_dollars else {
            return false;
        };
        let Some(current_pnl) = self.unrealized_pair_pnl(&d.pair_id, quotes) else {
            return false;
        };
        let Some(position) = self.active_pairs.get_mut(&d.pair_id) else {
            return false;
        };
        position.max_open_pnl = position.max_open_pnl.max(current_pnl);
        let retrace_fraction = d
            .profit_protection_retrace_fraction
            .unwrap_or(0.50)
            .clamp(0.0, 1.0);
        let floor = d.profit_protection_floor_dollars.unwrap_or(0.0).max(0.0);
        let trigger = (position.max_open_pnl * retrace_fraction).max(floor);
        position.max_open_pnl >= min_profit.max(0.0) && current_pnl <= trigger
    }

    fn unrealized_pair_pnl(&self, pair_id: &str, quotes: &HashMap<String, Quote>) -> Option<f64> {
        let position = self.active_pairs.get(pair_id)?;
        Some(unrealized_leg_pnl(&position.a, quotes)? + unrealized_leg_pnl(&position.b, quotes)?)
    }
}

fn position_qty(positions: &[Position], symbol: &str) -> f64 {
    positions
        .iter()
        .find(|p| p.symbol == symbol)
        .map(|p| p.qty)
        .unwrap_or(0.0)
}

fn sides_from_position(qty_a: f64, qty_b: f64) -> Option<(Side, Side)> {
    match (qty_a.is_sign_positive(), qty_b.is_sign_positive()) {
        (true, false) => Some((Side::Buy, Side::Sell)),
        (false, true) => Some((Side::Sell, Side::Buy)),
        _ => None,
    }
}

fn tracked_leg_for(order: &NewOrder, a: &str, b: &str) -> Result<TrackedLeg> {
    if order.symbol != a && order.symbol != b {
        return Err(anyhow!("{} does not belong to pair {a}/{b}", order.symbol));
    }
    Ok(TrackedLeg {
        symbol: order.symbol.clone(),
        qty: order.qty,
        side: order.side,
        entry_price: Some(order.limit_price),
    })
}

fn unrealized_leg_pnl(leg: &TrackedLeg, quotes: &HashMap<String, Quote>) -> Option<f64> {
    let entry_price = leg.entry_price?;
    let quote = quotes.get(&leg.symbol)?;
    let exit_price = aggressive_price(quote, leg.side.opposite());
    Some(match leg.side {
        Side::Buy => (exit_price - entry_price) * leg.qty,
        Side::Sell => (entry_price - exit_price) * leg.qty,
    })
}

fn passive_price(q: &Quote, side: Side) -> f64 {
    match side {
        Side::Buy => q.bid,
        Side::Sell => q.ask,
    }
}

fn aggressive_price(q: &Quote, side: Side) -> f64 {
    match side {
        Side::Buy => q.ask,
        Side::Sell => q.bid,
    }
}

pub fn balanced_quantities(target_leg_notional: f64, price_1: f64, price_2: f64) -> (f64, f64) {
    let max_qty_1 = ((target_leg_notional / price_1).ceil() as u64 + 3).max(1);
    let max_qty_2 = ((target_leg_notional / price_2).ceil() as u64 + 3).max(1);
    let mut best = (1, 1, f64::INFINITY);

    for qty_1 in 1..=max_qty_1 {
        for qty_2 in 1..=max_qty_2 {
            let notional_1 = qty_1 as f64 * price_1;
            let notional_2 = qty_2 as f64 * price_2;
            let imbalance = (notional_1 - notional_2).abs() / target_leg_notional;
            let target_drift = ((notional_1 - target_leg_notional).abs()
                + (notional_2 - target_leg_notional).abs())
                / (2.0 * target_leg_notional);
            let score = imbalance + 0.35 * target_drift;
            if score < best.2 {
                best = (qty_1, qty_2, score);
            }
        }
    }

    (best.0 as f64, best.1 as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balances_expensive_and_cheaper_etfs_near_target() {
        let (qqq_qty, xlk_qty) = balanced_quantities(2_000.0, 713.0, 178.0);

        assert_eq!(qqq_qty, 3.0);
        assert_eq!(xlk_qty, 12.0);
    }
}
