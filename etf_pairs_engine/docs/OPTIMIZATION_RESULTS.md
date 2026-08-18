# Optimization Results

Research run date: 2026-05-23

Data:

```text
source: Alpaca IEX historical bars
range: 2022-01-01 to 2026-05-22
symbols: QQQ, XLK, SMH, SPY, IWM, DIA, XLE, OIH, XLF, KRE, IYR, VNQ, EFA, EEM
timeframes tested: 1 hour, 4 hour, 1 day
slippage: 1.00 bps per fill
target leg notional: $2,000
```

The optimized model uses the stripped-down strategy:

```text
entry: abs(z_score) >= entry_zscore
exit: abs(z_score) <= exit_zscore
fallback exit: max_holding_bars
disabled: correlation filter, expected-move filter, spread-volatility filter, z-score stop, dollar stop, cooldown
```

## Best Mixed-Timeframe Strategy

Ranked by Rust-verified PnL:

| Pair | Timeframe | Beta | Window | Min Samples | Entry | Exit | Max Hold | Trades | Win Rate | PnL | Daily Sharpe |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| QQQ/SMH | 1d | 2.3375 | 60 | 45 | 2.00 | 0.00 | 90 | 3 | 66.67% | $3,587.58 | 1.19 |
| XLE/OIH | 1h | 1.6100 | 60 | 45 | 1.25 | 0.00 | 195 | 38 | 63.16% | $1,999.96 | 1.16 |
| XLF/KRE | 1d | 0.8750 | 60 | 45 | 1.75 | 0.00 | 90 | 9 | 77.78% | $1,681.87 | 1.18 |
| SPY/IWM | 1d | 3.5100 | 20 | 15 | 1.25 | 0.00 | 10 | 48 | 70.83% | $1,276.48 | 1.53 |
| XLK/SMH | 1d | 0.8160 | 20 | 15 | 1.50 | 0.00 | 40 | 22 | 68.18% | $1,178.00 | 0.80 |
| EFA/EEM | 1h | 1.8000 | 195 | 146 | 1.75 | 0.00 | 390 | 18 | 83.33% | $1,085.45 | 1.30 |
| SPY/QQQ | 1d | 0.7800 | 120 | 90 | 1.75 | 0.00 | 40 | 8 | 75.00% | $816.10 | 1.06 |
| DIA/SPY | 1d | 0.8500 | 20 | 15 | 1.75 | 0.00 | 90 | 7 | 85.71% | $763.81 | 1.20 |
| SPY/XLK | 4h | 2.2425 | 195 | 146 | 1.25 | 0.00 | 98 | 12 | 66.67% | $650.43 | 1.21 |
| IYR/VNQ | 1h | 1.0000 | 60 | 45 | 1.25 | 1.00 | 60 | 671 | 71.09% | $561.82 | 5.35 |
| QQQ/XLK | 1d | 3.7050 | 120 | 90 | 1.50 | 0.00 | 90 | 2 | 100.00% | $495.79 | 1.31 |

Portfolio-style aggregate across the selected pair models:

```text
trades: 838
wins: 596
win_rate: 71.12%
total_pnl: $14,097.29
avg_trade_pnl: $16.82
combined_daily_sharpe: 2.15
best_day: $1,989.85
worst_day: -$254.92
```

Annual verified PnL:

| Year | Trades | Win Rate | PnL | Daily Sharpe |
|---:|---:|---:|---:|---:|
| 2022 | 174 | 82.18% | $1,787.74 | 3.06 |
| 2023 | 218 | 78.44% | $2,641.56 | 3.17 |
| 2024 | 188 | 67.55% | $3,812.16 | 1.94 |
| 2025 | 194 | 61.86% | $2,115.80 | 3.00 |
| 2026 YTD | 64 | 54.69% | $3,740.03 | 2.85 |

## Evaluation

The highest-PnL strategy is mixed-timeframe and pair-specific. It materially outperformed the prior all-pair hourly baseline:

```text
previous best 2024+2025 all-hourly matrix: +$1,625.72
optimized 2022-2026 mixed-timeframe selected models: +$14,097.29
```

The main caveat is that several daily-bar winners have very few trades, especially `QQQ/SMH` and `QQQ/XLK`. They maximize historical PnL, but they are less statistically reliable than the higher-sample models such as `SPY/IWM`, `XLE/OIH`, `EFA/EEM`, and `IYR/VNQ`.

Operational note: live/paper mode now builds independent 1-hour, 4-hour, and daily signal bars from polled quotes when pairs define `signal_timeframe`. The optimized config is available at `config/optimized_mix.toml`. Live buckets are aligned to the New York regular session, and in-progress bars are persisted in SQLite when storage is enabled. The next robustness upgrade should be replacing quote polling with streaming market data and persisting fuller executor state across restarts.

## Output Files

```text
tmp_backtests/model_optimization_2022_2026/best_by_pair.csv
tmp_backtests/model_optimization_2022_2026/top_100_configs.csv
tmp_backtests/model_optimization_2022_2026/rust_verification.csv
tmp_backtests/model_optimization_2022_2026/verified_portfolio_summary.csv
tmp_backtests/model_optimization_2022_2026/verified_annual_summary.csv
tmp_backtests/model_optimization_2022_2026/verified_pair_summary.csv
```
