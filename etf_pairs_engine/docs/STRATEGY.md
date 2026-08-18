# Strategy Specification

## Pair model

```text
spread = price_A - beta * price_B
z_score = (spread - rolling_mean) / rolling_std
```

`beta` is fixed from the pair config, e.g. `beta = 2.70`.

## Signal interpretation

### Long spread

When spread is too low:

```text
A is weak relative to B
Trade: long A, short B
```

### Short spread

When spread is too high:

```text
A is strong relative to B
Trade: short A, long B
```

## Entry filters

Enter only if:

- `abs(z_score) >= entry_zscore`
- rolling sample count is sufficient
- both ETF quote spreads are below max spread bps
- no existing pair position is open
- pair notional is within limits
- short leg is tradable, shortable, and easy-to-borrow

## Exit filters

Exit when:

- spread converges, `abs(z_score) <= exit_zscore`
- max holding period is hit
- risk engine forces flattening

Convergence exits are implemented for paper/live polling mode and for backtests. Max-holding exits are implemented in the backtest and should still be wired into live/paper runtime risk exits.

## Sizing

The configured `leg_notional` is a target, not a direct order quantity. The executor searches nearby whole-share combinations and chooses quantities that make both legs as close as possible in dollar value while staying near the target notional.

Current active settings:

```toml
leg_notional = 2000.0
max_pair_notional = 5000.0
max_total_notional = 5000.0
```

For example, if QQQ is around 713 and XLK is around 178, the dynamic sizer prefers roughly:

```text
3 QQQ shares ~= 2139
12 XLK shares ~= 2136
```

## Initial recommended parameters

```toml
entry_zscore = 2.0
exit_zscore = 0.5
rolling_window_ticks = 3600
max_spread_bps = 8.0
```

## Pair-specific overrides

Global strategy settings are defaults. A pair can override its own window, z-score thresholds, quote-spread filter, and max holding period:

```toml
[[pairs]]
id = "SPY_IWM"
a = "SPY"
b = "IWM"
enabled = true
beta = 2.70
leg_notional = 2000.0
rolling_window_ticks = 390
min_samples = 293
entry_zscore = 1.50
exit_zscore = 0.50
max_holding_bars = 195
signal_timeframe = "1h"
```

When `signal_timeframe` is omitted or set to `tick`, the pair evaluates on every polling loop. When set to `1h`, `4h`, or `1d`, live/paper mode aggregates polled quotes into New York regular-session signal bars and updates that pair only once per completed bar. `rolling_window_ticks` and `max_holding_bars` then mean signal bars, not raw quote polls. In-progress bars are restored after restart when SQLite storage is enabled.

The optimized mixed-timeframe paper config is:

```bash
cargo run -- --config config/optimized_mix.toml run
```

## Walk-forward validation

The `walk-forward-csv` command is the preferred way to sanity-check whether a parameter family survives out-of-sample testing:

1. Train on a fixed historical slice.
2. Select the best parameter set from the built-in grid using `train_pnl - train_max_drawdown`, with a penalty for too few trades.
3. Test that selected parameter set on the next unseen slice.
4. Roll forward by one test slice and repeat.

This is stricter than a full-period backtest and should be weighted more heavily when deciding whether a pair is tradeable.

## First pair

```text
QQQ vs XLK
```

Rationale: liquid ETFs, related technology exposure, tight spreads, and a relationship that is easy to explain.
