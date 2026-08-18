# ETF Pairs Trading Engine

A Rust-first, low-latency ETF pairs trading engine designed for Alpaca paper trading, with a live-mode for possible addition once the strategy is further validated.

The initial strategy is a selective ETF basket/sector relative-value model. It trades temporary dislocations between related ETFs, using a fixed-beta rolling spread, z-score entry/exit logic, passive limit orders, shortability checks, and SQLite audit logging.

## Status

This is a working research and paper-trading engine under active development, not a finished prod trading system. It includes the Alpaca REST gateway, mock gateway, strategy engine, risk gates, passive order generation, audit DB tables, CSV/Alpaca backtesting, walk-forward validation, and analytics report generation. The biggest remaining improvement areas are streaming fill/order state management, fuller pair position tracking, live markouts monitoring, and an operator dashboard.

## Strategy summary

The engine models a pair as:

```text
spread = price_A - beta * price_B
z_score = (spread - rolling_mean) / rolling_std
```

Entry logic:

```text
z_score >= entry_zscore  -> short spread -> short A / long B
z_score <= -entry_zscore -> long spread  -> long A / short B
```

Exit logic:

```text
abs(z_score) <= exit_zscore -> spread has converged
```

The default config is intentionally conservative and still points at one paper-trading pair. Research configs can enable pair-specific strategy knobs.

## Architecture

```text
src/
  alpaca/      Broker gateway, Alpaca REST client, mock gateway
  pairs/       Spread, rolling stats, z-score and signal generation
  execution/   Pair order generation and shortability checks
  risk/        Portfolio and pair-level risk gates
  storage/     SQLite audit logging
  analytics/   Placeholder for future markout/convergence analytics
config/
  default.toml
  optimized_mix.toml
  conservative.toml
scripts/
  run_sim.sh
  check_assets.sh
  run_paper.sh
```

## Low-latency design choices

- Rust async runtime with Tokio
- Small symbol universe
- Fixed loop cadence with skipped missed ticks
- In-memory rolling windows using `VecDeque`
- Minimal allocations in the decision path where possible
- Shortability/easy-to-borrow checks before short ETF legs
- SQLite writes are lightweight and can be disabled in config
- Engine is intentionally one-level execution, not high-frequency quote stuffing

## Setup

```bash
cp .env.example .env
cargo build
```

Set your Alpaca paper keys:

```bash
export ALPACA_API_KEY="your_key"
export ALPACA_SECRET_KEY="your_secret"
```

## Run synthetic simulation

```bash
cargo run -- sim
```

This uses the mock gateway and synthetic quotes, with no Alpaca credentials needed.

## Check Alpaca asset status

```bash
cargo run -- check-assets
```

This prints whether each ETF is tradable, marginable, shortable, and easy-to-borrow. The engine refuses short ETF legs if the asset is not shortable and easy-to-borrow.

## Run against Alpaca paper

```bash
cargo run -- run
```

The default config points to Alpaca paper trading:

```toml
[alpaca]
paper = true
trading_base_url = "https://paper-api.alpaca.markets"
```

To paper trade the optimized mixed-timeframe research model:

```bash
cargo run -- --config config/optimized_mix.toml run
```

To paper trade the stricter conservative RTH-validated candidate:

```bash
cargo run -- --config config/conservative.toml run
```

Pairs with `signal_timeframe = "1h"`, `"4h"`, or `"1d"` are driven by completed Alpaca bars in paper/live mode when `signal_bar_source = "alpaca"`. Tick-style pairs without `signal_timeframe` still evaluate on every polling loop.

## Which Config Should I Use?

`config/default.toml` is intentionally cautious. It enables only `QQQ/XLK`, caps open pairs at one, and is meant as the safe first-run/paper-mode default. The single enabled pair is not the full tested strategy universe.

Use these configs depending on what you are trying to show or run:

| config | role | enabled pairs |
| --- | --- | --- |
| `config/default.toml` | Minimal demo and first paper run | `QQQ/XLK` |
| `config/optimized_mix.toml` | Latest broader optimized mixed-timeframe research candidate | `QQQ/SMH`, `XLE/OIH`, `XLF/KRE`, `SPY/IWM`, `XLK/SMH`, `EFA/EEM`, `SPY/QQQ`, `DIA/SPY`, `SPY/XLK`, `IYR/VNQ`, `QQQ/XLK` |
| `config/conservative.toml` | Stricter RTH holdout candidate for cautious paper testing | `SPY/IWM`, `QQQ/XLK` |
| `config/six_pair_vol_adjusted_4h.toml` | Six-pair volatility-adjusted validation config | `XLE/OIH`, `SPY/IWM`, `SPY/QQQ`, `QQQ/XLK`, `DIA/SPY`, `EFA/EEM` |

So if you only see `QQQ/XLK`, you are looking at the safe default config, not the research results as a whole.

## Pair-specific Strategy Overrides

The global `[strategy]` block provides defaults. Any enabled `[[pairs]]` entry can override the key strategy fields:

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
min_expected_spread_move_after_costs = 25.0
adverse_zscore = 1.50
stop_loss_dollars = 100.0
```

For local hourly research, start from:

```bash
cargo run -- --config config/hourly_research.toml backtest \
  --from 2025-01-01 \
  --to 2025-12-31 \
  --timeframe 1Hour \
  --feed iex
```

For a one-off CSV backtest:

```bash
cargo run -- backtest-csv \
  --bars-csv data/focus_pairs_1hour_2022-01-01_2026-05-22.csv \
  --pair-id SPY_IWM \
  --a SPY \
  --b IWM \
  --beta 2.70 \
  --window 390 \
  --min-samples 293 \
  --entry-zscore 1.50 \
  --exit-zscore 0.50 \
  --max-holding-bars 195 \
  --min-expected-spread-move-after-costs 25 \
  --adverse-zscore 1.5 \
  --stop-loss-dollars 100
```

## Walk-Forward Validation

Use walk-forward validation to reduce in-sample parameter luck. The command optimizes on one training slice, chooses the best config from a compact hourly grid, then tests only on the next unseen slice.

```bash
cargo run -- walk-forward-csv \
  --bars-csv data/focus_pairs_1hour_2022-01-01_2026-05-22.csv \
  --pair-id SPY_IWM \
  --a SPY \
  --b IWM \
  --beta 2.70 \
  --train-bars 2016 \
  --test-bars 504 \
  --min-train-trades 5 \
  --output-csv tmp_backtests/walk_forward_2022_2026_focus/SPY_IWM.csv
```

The default grid tests hourly-relevant windows, z-score thresholds, and max-holding settings around the current research setup. Treat walk-forward results as more important than full-period in-sample backtests.

## Optimized Research Strategy

The highest-PnL research pass currently uses pair-specific timeframes across hourly, 4-hour, and daily bars. The runnable paper config is [config/optimized_mix.toml](config/optimized_mix.toml). See [docs/OPTIMIZATION_RESULTS.md](docs/OPTIMIZATION_RESULTS.md) for the tested parameters, Rust verification, annual PnL, and Sharpe metrics.

To rerun the optimized mixed-timeframe backtests:

```bash
etf_pairs_engine/scripts/run_optimized_backtests.sh
```

Live/paper mode now polls Alpaca for completed native `1Hour`, `4Hour`, and `1Day` bars and feeds each pair only when a new common completed bar timestamp exists across the required symbols. Orders are still priced from the latest raw quote snapshot.

## Conservative RTH Strategy

The stricter validation pass uses minute bars aggregated into regular-session `1h`, `4h`, and `1d` bars. See [docs/CONSERVATIVE_RTH_VALIDATION.md](docs/CONSERVATIVE_RTH_VALIDATION.md).

The conservative config is [config/conservative.toml](config/conservative.toml). It keeps only `SPY/IWM` and `QQQ/XLK` enabled by default because `DIA/SPY` and `SPY/XLK` failed the 2026 holdout after Rust verification.

To rerun the local conservative optimization:

```bash
python3 etf_pairs_engine/scripts/optimize_conservative_rth.py
```

## Engineering enhancement checklist

1. Add streaming market data instead of REST polling.
2. Add streaming trade/order updates.
3. Replace basic pair submission with a fill-aware state machine:
   - Idle
   - SignalDetected
   - Leg1Pending
   - Leg1Filled
   - Leg2Pending
   - PositionOpen
   - ExitPending
   - Flattening
   - Closed
   - RiskExit
4. Add position reconciliation from Alpaca account positions.
5. Add explicit cancel and stale-order management.
6. Add aggressive rescue logic after max leg delay.
7. Add live markout tables and scheduled paper/live markout sampling.
8. Add a TypeScript GUI / Tauri dash.
9. Add macro-event calendar blocks.
10. Add end-to-end integration tests with a mocked Alpaca server.

## Disclaimer

This is research and paper-trading infrastructure under active development. It is not financial advice and is not production-ready trading infrastructure.


Run these from:

bash
cd "/Users/andrej/Desktop/Trading Strategy Folder/ETF Pairs Trading"


Use simulation first:

bash
cargo run -- sim

Check Alpaca ETF tradability/shortability:

bash
cargo run -- check-assets

Run against Alpaca paper mode:

bash
cargo run -- run

To stop sim or run, press Ctrl+c.

`config/default.toml` intentionally enables only `QQQ/XLK` for a first-run universe. Use `config/optimized_mix.toml` to inspect the broader optimized candidate, or `config/conservative.toml` for the stricter paper/live candidate.

How to pull data (example of one min data for various ETFs for a whole year):

cargo run -- download-bars \
  --from 2025-05-12 \
  --to 2026-05-12 \
  --timeframe 1Min \
  --symbols tested \
  --feed iex \
  --limit 10000 \
  --output data/tested_etfs_1min_2025-05-12_2026-05-12.csv
