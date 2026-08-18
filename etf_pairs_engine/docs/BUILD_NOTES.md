# Build Notes

Recommended local verification commands:

```bash
cargo fmt --check
cargo check
cargo test
cargo clippy --all-targets -- -D warnings
cargo run -- check-assets
cargo run -- backtest-csv --bars-csv examples/sample_pairs_bars.csv --pair-id DEMO_PAIR --a DEMOA --b DEMOB --beta 1.0 --window 5 --min-samples 5 --entry-zscore 1.0 --exit-zscore 0.25 --max-holding-bars 4 --leg-notional 1000 --slippage-bps 1 --report-dir tmp_reports/demo_backtest
```

Remaining engineering tasks:

1. Replace order polling with Alpaca trade update streaming.
2. Add persistent pair/order state snapshots.
3. Add open-order reconciliation on startup.
4. Add integration tests around a mocked Alpaca HTTP server.
5. Replace REST polling with streaming market data for lower latency.
6. Improve the backtest with quote/trade replay.
7. Extend generated analytics with live/paper markout sampling.
