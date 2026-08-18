# Conservative RTH Validation

This research pass used the minute-derived regular-session bars from:

`data/tested_etfs_1min_2025-05-12_2026-05-12.csv`

The minute bars were aggregated into NYSE regular-session `1h`, `4h`, and `1d`
bars before testing. This is more faithful to the live mixed-timeframe bar
aggregator than using Alpaca's native hourly bars.

## Split

- Train/optimize: `2025-05-12` through `2025-12-31`
- Test/validate: `2026-01-01` through `2026-05-12`
- Slippage: `1 bp` per fill
- Sizing: dynamic whole-share sizing targeting roughly `$2,000` per leg

## Controls Added

- `min_expected_spread_move_after_costs`: blocks entries where the expected move
  from entry z-score toward exit z-score is too small.
- `adverse_zscore`: exits when the spread moves further against the entry by the
  configured z-score amount.
- `stop_loss_dollars`: exits in the backtester when unrealized pair PnL breaches
  the configured dollar loss threshold.

Live caveat: `adverse_zscore` and the entry filter are available to live paper
mode. Dollar stop behavior is exact in the simulator; live dollar-stop precision
still depends on the fill-aware position ledger being fully productionized.

## Rust-Verified Walk-Forward Result

Best train-selected candidates verified in Rust:

| split | pair | timeframe | trades | wins | win rate | PnL | max drawdown |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: |
| train | SPY_XLK | 1d | 3 | 3 | 100.00% | 237.43 | 0.00 |
| train | SPY_IWM | 4h | 8 | 7 | 87.50% | 351.76 | 21.05 |
| train | QQQ_XLK | 4h | 13 | 9 | 69.23% | 121.28 | 8.85 |
| train | DIA_SPY | 1d | 5 | 5 | 100.00% | 104.26 | 0.00 |
| test | SPY_XLK | 1d | 2 | 1 | 50.00% | -76.49 | 104.63 |
| test | SPY_IWM | 4h | 4 | 2 | 50.00% | 25.53 | 28.24 |
| test | QQQ_XLK | 4h | 5 | 4 | 80.00% | 17.31 | 7.66 |
| test | DIA_SPY | 1d | 5 | 3 | 60.00% | -24.66 | 40.92 |

Portfolio totals:

| split | trades | wins | win rate | PnL |
| --- | ---: | ---: | ---: | ---: |
| train | 29 | 24 | 82.76% | 814.73 |
| test | 16 | 10 | 62.50% | -58.31 |

## Interpretation

The full four-pair conservative universe still overfit the training window.
Only `SPY_IWM` and `QQQ_XLK` produced positive test PnL under the Rust verifier.
For that reason, `config/conservative.toml` enables only those two pairs by
default and leaves `DIA_SPY` and `SPY_XLK` present but disabled.

This is a defensive paper-trading candidate, not a proven production model. The
main next step is more out-of-sample RTH minute data, ideally multiple years,
before increasing size or allowing unattended trading.

## Alpaca-Native Bar Follow-Up

Paper/live mode can now poll Alpaca completed bars directly for `1h`, `4h`, and
`1d` signals. That makes native Alpaca bar backtests consistent with live signal
generation, avoiding the previous mismatch between native hourly backtests and
internally aggregated quote bars.

Using Alpaca IEX native bars over the same validation split:

| split | enabled set | trades | wins | win rate | PnL |
| --- | --- | ---: | ---: | ---: | ---: |
| train | SPY/IWM + QQQ/XLK | 21 | 13 | 61.90% | 260.19 |
| test | SPY/IWM + QQQ/XLK | 13 | 7 | 53.85% | 13.29 |
| train | all four candidates | 28 | 19 | 67.86% | 342.26 |
| test | all four candidates | 20 | 11 | 55.00% | -87.83 |

The Alpaca-native validation still supports keeping only `SPY/IWM` and
`QQQ/XLK` enabled by default. The positive test PnL is small, so this remains a
paper-only candidate.

## Enhanced Alpaca-Bar Candidate

The next pass added:

- Rolling beta with clamp ranges.
- Optional minimum rolling correlation.
- Optional entry confirmation after the initial z-score breach.
- Existing expected-move, adverse-z, and dollar-stop controls.

Only the enabled `SPY/IWM` and `QQQ/XLK` pairs were retested on native Alpaca
`4Hour` bars, using the same train/test split.

| split | pair | controls | trades | wins | win rate | PnL | max drawdown |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: |
| train | SPY/IWM | confirmation + corr gate | 5 | 5 | 100.00% | 414.47 | 0.00 |
| train | QQQ/XLK | rolling beta | 7 | 5 | 71.43% | 144.85 | 23.91 |
| test | SPY/IWM | confirmation + corr gate | 3 | 2 | 66.67% | 64.72 | 0.77 |
| test | QQQ/XLK | rolling beta | 6 | 5 | 83.33% | 93.33 | 22.18 |

Portfolio totals:

| split | trades | wins | win rate | PnL |
| --- | ---: | ---: | ---: | ---: |
| train | 12 | 10 | 83.33% | 559.32 |
| test | 9 | 7 | 77.78% | 158.05 |

This is better than the prior Alpaca-native holdout result of `+$13.29`, but the
sample is still very small. Treat the enhanced config as a paper-trading
candidate only.
