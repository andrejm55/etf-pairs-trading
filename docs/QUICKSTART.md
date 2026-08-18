# Quickstart

## Prerequisites

- Rust stable toolchain
- Optional: Alpaca paper-trading credentials for broker-connected commands

## Build And Test

```bash
cd etf_pairs_engine
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Run Simulation

```bash
cargo run -- sim
```

Simulation uses the mock gateway and does not need API credentials.

## Run The Public Demo Backtest

```bash
cargo run -- backtest-csv \
  --bars-csv examples/sample_pairs_bars.csv \
  --pair-id DEMO_PAIR \
  --a DEMOA \
  --b DEMOB \
  --beta 1.0 \
  --window 5 \
  --min-samples 5 \
  --entry-zscore 1.0 \
  --exit-zscore 0.25 \
  --max-holding-bars 4 \
  --leg-notional 1000 \
  --slippage-bps 1 \
  --report-dir tmp_reports/demo_backtest
```

The sample data is synthetic and exists only to verify the command path and analytics generation. It should not be interpreted as market evidence.

## Generate A Walk-Forward Report

```bash
cargo run -- walk-forward-csv \
  --bars-csv examples/sample_pairs_bars.csv \
  --pair-id DEMO_PAIR \
  --a DEMOA \
  --b DEMOB \
  --beta 1.0 \
  --train-bars 5 \
  --test-bars 3 \
  --min-train-trades 0 \
  --leg-notional 1000 \
  --slippage-bps 1 \
  --output-csv tmp_reports/demo_walk_forward.csv \
  --report-dir tmp_reports/demo_walk_forward
```

For real research, use longer historical data and stricter minimum-trade thresholds.

## Paper Trading

```bash
cp ../.env.example .env
```

Edit `.env` with Alpaca paper credentials, then:

```bash
cargo run -- check-assets
cargo run -- --config config/conservative.toml run
```
