# Architecture

## Objective

Build a small-universe, low-latency ETF relative-value engine for Alpaca paper trading. The system is designed for selective opportunities, not continuous high-frequency market making.

## Data path

```text
Alpaca REST / Mock quotes / Historical bars
        ↓
Quote normalisation
        ↓
Rolling pair spread engine
        ↓
Risk filters
        ↓
Execution decision
        ↓
Pair executor / backtest simulator
        ↓
SQLite audit trail
```

## Strategy path

For each enabled pair:

1. Read latest A/B quotes.
2. For `tick` pairs, evaluate immediately on the polling loop.
3. For `1h`, `4h`, and `1d` pairs, aggregate polled quotes into completed signal bars first.
4. Compute mid prices from the tick quote or completed signal bar close.
5. Resolve pair-specific strategy settings from global defaults plus pair overrides.
6. Compute spread with the configured fixed beta: `A_mid - beta * B_mid`.
7. Update the rolling spread window.
8. Compute rolling mean, std and z-score.
9. Compute rolling return correlation for audit/diagnostics.
10. Apply quote-spread and risk filters.
11. Emit decision.

The bar aggregator uses New York regular-session buckets. Hourly bars are anchored to 9:30 ET, 4-hour bars to 9:30 ET and 13:30 ET, and daily bars to the regular-session date. A completed bar is emitted only when a new bucket starts, so the final intraday or daily bar is emitted on the next regular-session quote. Execution still uses the latest raw quote snapshot, not the bar-close quote, for order pricing.

## Execution path

The paper executor submits paired limit orders and manages a lightweight pair lifecycle:

- Entry orders can be passive or aggressive based on config.
- Exit and rescue orders cross the spread using aggressive limit prices.
- Short ETF legs require `tradable=true`, `shortable=true`, and `easy_to_borrow=true`.
- Both entry legs are polled until filled, terminal, or timed out.
- The pair is marked open only after both entry legs fill.
- Stale/incomplete legs are cancelled.
- If one entry leg fills and the pair cannot complete, the executor attempts to flatten the filled leg.
- Exit orders are submitted when convergence is detected.
- On startup, Alpaca open positions are queried and enabled pair state is rebuilt when both legs are present.

The next version should replace polling with Alpaca streaming trade updates and persist fuller executor state snapshots.

## Target state machine

```text
Idle
SignalDetected
Leg1Pending
Leg1Filled
Leg2Pending
PositionOpen
ExitPending
Flattening
Closed
RiskExit
```

## Risk path

Initial risk gates:

- max pair notional
- max open pairs
- max daily loss
- max total notional
- shortability/easy-to-borrow requirement
- spread width filter
- startup position reconciliation

Planned risk gates:

- max open order count
- max cancel count
- max realised loss by pair
- manual kill switch

## Backtest path

Historical bars are fetched from Alpaca and aligned by timestamp. The backtest uses the same `PairEngine` signal path as paper mode, applies dynamic whole-share sizing, estimates fills around bar closes with configurable slippage, tracks realised PnL, closes on convergence or max holding bars, and can write a trade CSV.

Pair-specific `max_holding_bars` overrides are honored in backtests and in live/paper mode. In live/paper mode, max holding is counted in completed signal bars for each pair's configured `signal_timeframe`.

Walk-forward CSV validation reuses the same simulator. It slices aligned historical bars into train/test windows, optimizes parameters on the train window, and evaluates the selected parameters on the next unseen test window.

## Storage

SQLite is used for local research and paper-mode auditability.

Tables:

- `pair_signals`
- `pair_orders`
- `pair_execution`
- `risk_events`
- `bar_aggregator_state`

Postgres can replace SQLite later with the same logical schema.
