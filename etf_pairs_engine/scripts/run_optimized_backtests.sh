#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ENGINE="$ROOT/target/debug/etf_pairs_engine"
OUT="$ROOT/tmp_backtests/optimized_strategy_rerun"

mkdir -p "$OUT"

if [[ ! -x "$ENGINE" ]]; then
  cargo build --manifest-path "$ROOT/Cargo.toml"
fi

run_pair() {
  local pair_id="$1"
  local a="$2"
  local b="$3"
  local beta="$4"
  local timeframe="$5"
  local csv_path="$6"
  local window="$7"
  local min_samples="$8"
  local entry="$9"
  local exit_z="${10}"
  local hold="${11}"

  "$ENGINE" backtest-csv \
    --bars-csv "$csv_path" \
    --pair-id "$pair_id" \
    --a "$a" \
    --b "$b" \
    --beta "$beta" \
    --leg-notional 2000 \
    --window "$window" \
    --min-samples "$min_samples" \
    --entry-zscore "$entry" \
    --exit-zscore "$exit_z" \
    --max-holding-bars "$hold" \
    --slippage-bps 1.0 \
    --trades-csv "$OUT/${pair_id}_${timeframe}_trades.csv" \
    > "$OUT/${pair_id}_${timeframe}.txt"
}

H1="$ROOT/data/tested_etfs_1hour_2022-01-01_2026-05-22.csv"
H4="$ROOT/data/tested_etfs_4hour_2022-01-01_2026-05-22.csv"
D1="$ROOT/data/tested_etfs_1day_2022-01-01_2026-05-22.csv"

run_pair QQQ_SMH QQQ SMH 2.3375 1d "$D1" 60 45 2.00 0.00 90
run_pair XLE_OIH XLE OIH 1.6100 1h "$H1" 60 45 1.25 0.00 195
run_pair XLF_KRE XLF KRE 0.8750 1d "$D1" 60 45 1.75 0.00 90
run_pair SPY_IWM SPY IWM 3.5100 1d "$D1" 20 15 1.25 0.00 10
run_pair XLK_SMH XLK SMH 0.8160 1d "$D1" 20 15 1.50 0.00 40
run_pair EFA_EEM EFA EEM 1.8000 1h "$H1" 195 146 1.75 0.00 390
run_pair SPY_QQQ SPY QQQ 0.7800 1d "$D1" 120 90 1.75 0.00 40
run_pair DIA_SPY DIA SPY 0.8500 1d "$D1" 20 15 1.75 0.00 90
run_pair SPY_XLK SPY XLK 2.2425 4h "$H4" 195 146 1.25 0.00 98
run_pair IYR_VNQ IYR VNQ 1.0000 1h "$H1" 60 45 1.25 1.00 60
run_pair QQQ_XLK QQQ XLK 3.7050 1d "$D1" 120 90 1.50 0.00 90

cat "$OUT"/*.txt > "$OUT/reports.txt"
echo "Wrote $OUT/reports.txt"
