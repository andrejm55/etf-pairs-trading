# Six-Pair Volatility-Adjusted Results

Research run date: 2026-07-12

This note records the best verified six-pair Alpaca-native 4-hour result so far,
without committing raw historical data or generated trade CSVs.

## Scope

Data:

```text
source: Alpaca IEX native 4-hour bars
range: 2022-01-03 to 2026-07-06
slippage: 1.00 bps per fill
```

Pairs:

```text
XLE/OIH
SPY/IWM
SPY/QQQ
QQQ/XLK
DIA/SPY
EFA/EEM
```

The executable config is:

```text
config/six_pair_vol_adjusted_4h.toml
```

## Baseline Comparison

| Variant | Trades | Wins | Win Rate | PnL | Max Drawdown | PnL/DD |
|---|---:|---:|---:|---:|---:|---:|
| Original fixed six-pair basket | 333 | 194 | 58.26% | $2,520.83 | $529.35 | 4.76 |
| Volatility-adjusted sizing | 332 | 194 | 58.43% | $2,896.18 | $485.76 | 5.96 |
| Volatility-adjusted + best profit protection | 336 | 198 | 58.93% | $2,977.65 | $493.23 | 6.04 |

The best verified variant so far is volatility-adjusted sizing with profit
protection enabled only on the confirmation-style pairs:

```text
XLE/OIH
SPY/IWM
DIA/SPY
```

Profit protection settings:

```text
profit_protection_min_dollars = 50.0
profit_protection_retrace_fraction = 0.50
profit_protection_floor_dollars = 10.0
```

## Year-by-Year PnL

| Year | Trades | Wins | Win Rate | PnL | Max Drawdown | PnL/DD |
|---:|---:|---:|---:|---:|---:|---:|
| 2022 | 78 | 42 | 53.85% | $330.73 | $459.06 | 0.72 |
| 2023 | 78 | 41 | 52.56% | $47.12 | $493.23 | 0.10 |
| 2024 | 67 | 42 | 62.69% | $577.34 | $224.18 | 2.58 |
| 2025 | 71 | 45 | 63.38% | $1,123.80 | $208.87 | 5.38 |
| 2026 YTD | 42 | 28 | 66.67% | $898.66 | $233.64 | 3.85 |

## Pair-Level PnL

| Pair | Trades | Wins | Win Rate | PnL | Max Drawdown | PnL/DD |
|---|---:|---:|---:|---:|---:|---:|
| XLE/OIH | 47 | 30 | 63.83% | $907.77 | $276.82 | 3.28 |
| SPY/IWM | 43 | 24 | 55.81% | $214.56 | $212.89 | 1.01 |
| SPY/QQQ | 79 | 46 | 58.23% | $758.38 | $154.16 | 4.92 |
| QQQ/XLK | 74 | 43 | 58.11% | $460.99 | $178.05 | 2.59 |
| DIA/SPY | 19 | 13 | 68.42% | $474.12 | $104.39 | 4.54 |
| EFA/EEM | 74 | 42 | 56.76% | $161.83 | $205.58 | 0.79 |

## Evaluation

The profit-protection variant adds about `$456.82` PnL versus the original
fixed six-pair basket and improves PnL/DD from `4.76` to `6.04`.

The weak spot remains 2023. The model stays profitable that year, but only
modestly, and the year carries the highest drawdown in the test. This result is
better than prior portfolio-wide risk overlays because it improves total PnL
instead of trading away too much upside for smoothness.

Raw generated results were intentionally left out of git. The local verification
outputs were generated under:

```text
tmp_backtests/vol_adjusted_rust_4h_2022_2026_scaled_filters/
tmp_backtests/profit_protection_pair_specific_rust_4h_2022_2026/
```
