#!/usr/bin/env python3
"""Research sweep for ETF pair parameters across hourly, 4-hour, and daily bars.

The simulator mirrors the current stripped-down Rust model:
- fixed beta spread: A - beta * B
- entry on +/- z-score threshold
- exit on convergence or max holding bars
- dynamic whole-share balanced sizing
- close-price fills with configurable slippage
"""

from __future__ import annotations

import csv
import itertools
import math
import subprocess
from dataclasses import dataclass
from pathlib import Path

import numpy as np
import pandas as pd


ROOT = Path(__file__).resolve().parents[2]
HOURLY_CSV = ROOT / "data" / "tested_etfs_1hour_2022-01-01_2026-05-22.csv"
FOUR_HOUR_CSV = ROOT / "data" / "tested_etfs_4hour_2022-01-01_2026-05-22.csv"
DAILY_CSV = ROOT / "data" / "tested_etfs_1day_2022-01-01_2026-05-22.csv"
OUT_DIR = ROOT / "tmp_backtests" / "model_optimization_2022_2026"
ENGINE = ROOT / "target" / "debug" / "etf_pairs_engine"

LEG_NOTIONAL = 2000.0
SLIPPAGE_BPS = 1.0

PAIRS = [
    ("QQQ_XLK", "QQQ", "XLK", 2.85),
    ("QQQ_SMH", "QQQ", "SMH", 2.75),
    ("XLK_SMH", "XLK", "SMH", 0.96),
    ("SPY_QQQ", "SPY", "QQQ", 0.78),
    ("SPY_XLK", "SPY", "XLK", 1.95),
    ("SPY_IWM", "SPY", "IWM", 2.70),
    ("DIA_SPY", "DIA", "SPY", 0.85),
    ("XLE_OIH", "XLE", "OIH", 1.40),
    ("XLF_KRE", "XLF", "KRE", 1.25),
    ("IYR_VNQ", "IYR", "VNQ", 1.00),
    ("EFA_EEM", "EFA", "EEM", 1.80),
]

GRIDS = {
    "1h": {
        "csv": HOURLY_CSV,
        "windows": [60, 195, 390, 780],
        "holds": [60, 195, 390],
    },
    "4h": {
        "csv": FOUR_HOUR_CSV,
        "windows": [30, 98, 195],
        "holds": [20, 60, 98],
    },
    "1d": {
        "csv": DAILY_CSV,
        "windows": [20, 60, 120, 252],
        "holds": [10, 40, 90],
    },
}

ENTRIES = [1.25, 1.50, 1.75, 2.00]
EXITS = [0.00, 0.50, 1.00]
BETA_MULTIPLIERS = [0.70, 0.85, 1.00, 1.15, 1.30]
TOP_STAGE1_PER_PAIR_TF = 5


@dataclass(frozen=True)
class Params:
    timeframe: str
    pair_id: str
    a: str
    b: str
    beta: float
    window: int
    min_samples: int
    entry: float
    exit: float
    hold: int


def ensure_resampled_files() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    if FOUR_HOUR_CSV.exists() and DAILY_CSV.exists():
        return

    raw = pd.read_csv(HOURLY_CSV, parse_dates=["t"])
    raw = raw.sort_values(["symbol", "t"])

    if not FOUR_HOUR_CSV.exists():
        frames = []
        for symbol, group in raw.groupby("symbol", sort=True):
            group = group.copy()
            group["session"] = group["t"].dt.date
            group["bucket"] = group.groupby("session").cumcount() // 4
            agg = (
                group.groupby(["session", "bucket"], sort=True)
                .agg(
                    symbol=("symbol", "first"),
                    t=("t", "first"),
                    o=("o", "first"),
                    h=("h", "max"),
                    l=("l", "min"),
                    c=("c", "last"),
                    v=("v", "sum"),
                    n=("n", "sum"),
                    vw=("vw", "mean"),
                )
                .reset_index(drop=True)
            )
            frames.append(agg)
        write_bars_csv(pd.concat(frames, ignore_index=True), FOUR_HOUR_CSV)

    if not DAILY_CSV.exists():
        frames = []
        for symbol, group in raw.groupby("symbol", sort=True):
            group = group.copy()
            group["session"] = group["t"].dt.date
            agg = (
                group.groupby("session", sort=True)
                .agg(
                    symbol=("symbol", "first"),
                    t=("t", "first"),
                    o=("o", "first"),
                    h=("h", "max"),
                    l=("l", "min"),
                    c=("c", "last"),
                    v=("v", "sum"),
                    n=("n", "sum"),
                    vw=("vw", "mean"),
                )
                .reset_index(drop=True)
            )
            frames.append(agg)
        write_bars_csv(pd.concat(frames, ignore_index=True), DAILY_CSV)


def write_bars_csv(df: pd.DataFrame, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    out = df[["symbol", "t", "o", "h", "l", "c", "v", "n", "vw"]].copy()
    out["t"] = pd.to_datetime(out["t"], utc=True).dt.strftime("%Y-%m-%dT%H:%M:%S%z")
    out["t"] = out["t"].str.replace(r"(\+0000)$", "+00:00", regex=True)
    out.to_csv(path, index=False, float_format="%.6f")


def load_close_matrix(path: Path) -> pd.DataFrame:
    df = pd.read_csv(path, parse_dates=["t"], usecols=["symbol", "t", "c"])
    matrix = df.pivot(index="t", columns="symbol", values="c").sort_index()
    return matrix


def balanced_quantities(target: float, price_1: float, price_2: float) -> tuple[int, int]:
    max_qty_1 = max(int(math.ceil(target / price_1)) + 3, 1)
    max_qty_2 = max(int(math.ceil(target / price_2)) + 3, 1)
    best = (1, 1, float("inf"))
    for qty_1 in range(1, max_qty_1 + 1):
        for qty_2 in range(1, max_qty_2 + 1):
            notional_1 = qty_1 * price_1
            notional_2 = qty_2 * price_2
            imbalance = abs(notional_1 - notional_2) / target
            target_drift = (abs(notional_1 - target) + abs(notional_2 - target)) / (2.0 * target)
            score = imbalance + 0.35 * target_drift
            if score < best[2]:
                best = (qty_1, qty_2, score)
    return best[0], best[1]


def fill_price(mid: float, side: str, slippage_bps: float) -> float:
    slip = slippage_bps / 10_000.0
    return mid * (1.0 + slip) if side == "buy" else mid * (1.0 - slip)


def leg_pnl(side: str, entry: float, exit_price: float, qty: int) -> float:
    if side == "buy":
        return (exit_price - entry) * qty
    return (entry - exit_price) * qty


def simulate_pair(closes: pd.DataFrame, params: Params) -> dict:
    data = closes[[params.a, params.b]].dropna()
    if len(data) < params.min_samples + 2:
        return empty_result(params, len(data))

    a_prices = data[params.a].to_numpy(dtype=float)
    b_prices = data[params.b].to_numpy(dtype=float)
    index = data.index
    spread = pd.Series(a_prices - params.beta * b_prices, index=index)
    mean = spread.rolling(params.window, min_periods=params.min_samples).mean()
    std = spread.rolling(params.window, min_periods=params.min_samples).std(ddof=1).clip(lower=1e-9)
    z = ((spread - mean) / std).to_numpy(dtype=float)

    open_pos = None
    trades = []
    slip = SLIPPAGE_BPS

    for i, z_value in enumerate(z):
        if math.isnan(z_value):
            continue
        a_mid = a_prices[i]
        b_mid = b_prices[i]
        ts = index[i]

        if open_pos is not None:
            hold_bars = i - open_pos["open_i"]
            if hold_bars >= params.hold or abs(z_value) <= params.exit:
                exit_a = fill_price(a_mid, "sell" if open_pos["side_a"] == "buy" else "buy", slip)
                exit_b = fill_price(b_mid, "sell" if open_pos["side_b"] == "buy" else "buy", slip)
                pnl = leg_pnl(open_pos["side_a"], open_pos["entry_a"], exit_a, open_pos["qty_a"])
                pnl += leg_pnl(open_pos["side_b"], open_pos["entry_b"], exit_b, open_pos["qty_b"])
                trades.append(
                    {
                        "opened_at": open_pos["opened_at"],
                        "closed_at": ts,
                        "holding_bars": hold_bars,
                        "entry_z": open_pos["entry_z"],
                        "exit_z": z_value,
                        "pnl": pnl,
                        "reason": "max_holding_bars" if hold_bars >= params.hold else "zscore_converged",
                    }
                )
                open_pos = None
                continue

        if open_pos is None:
            if z_value >= params.entry:
                side_a, side_b = "sell", "buy"
            elif z_value <= -params.entry:
                side_a, side_b = "buy", "sell"
            else:
                continue
            qty_a, qty_b = balanced_quantities(LEG_NOTIONAL, a_mid, b_mid)
            open_pos = {
                "open_i": i,
                "opened_at": ts,
                "entry_z": z_value,
                "side_a": side_a,
                "side_b": side_b,
                "qty_a": qty_a,
                "qty_b": qty_b,
                "entry_a": fill_price(a_mid, side_a, slip),
                "entry_b": fill_price(b_mid, side_b, slip),
            }

    if open_pos is not None:
        i = len(index) - 1
        a_mid = a_prices[i]
        b_mid = b_prices[i]
        exit_a = fill_price(a_mid, "sell" if open_pos["side_a"] == "buy" else "buy", slip)
        exit_b = fill_price(b_mid, "sell" if open_pos["side_b"] == "buy" else "buy", slip)
        pnl = leg_pnl(open_pos["side_a"], open_pos["entry_a"], exit_a, open_pos["qty_a"])
        pnl += leg_pnl(open_pos["side_b"], open_pos["entry_b"], exit_b, open_pos["qty_b"])
        trades.append(
            {
                "opened_at": open_pos["opened_at"],
                "closed_at": index[i],
                "holding_bars": i - open_pos["open_i"],
                "entry_z": open_pos["entry_z"],
                "exit_z": z[i] if not math.isnan(z[i]) else 0.0,
                "pnl": pnl,
                "reason": "end_of_backtest",
            }
        )

    return result_from_trades(params, len(data), data.index, trades)


def empty_result(params: Params, bars: int) -> dict:
    return {
        **params.__dict__,
        "bars": bars,
        "trades": 0,
        "wins": 0,
        "win_rate": 0.0,
        "total_pnl": 0.0,
        "avg_trade_pnl": 0.0,
        "max_drawdown": 0.0,
        "daily_sharpe": 0.0,
        "trade_sharpe": 0.0,
    }


def result_from_trades(params: Params, bars: int, index: pd.DatetimeIndex, trades: list[dict]) -> dict:
    pnls = np.array([t["pnl"] for t in trades], dtype=float)
    total_pnl = float(pnls.sum()) if len(pnls) else 0.0
    wins = int((pnls > 0).sum()) if len(pnls) else 0
    equity = np.cumsum(pnls) if len(pnls) else np.array([], dtype=float)
    max_dd = max_drawdown(equity)
    trade_sharpe = sharpe(pnls, periods=math.sqrt(len(pnls))) if len(pnls) > 1 else 0.0

    daily = pd.Series(0.0, index=pd.Index(sorted({ts.date() for ts in index}), name="pnl"))
    for trade in trades:
        daily.loc[trade["closed_at"].date()] += trade["pnl"]
    daily_sharpe = sharpe(daily.to_numpy(dtype=float), periods=math.sqrt(252))

    return {
        **params.__dict__,
        "bars": bars,
        "trades": len(trades),
        "wins": wins,
        "win_rate": wins / len(trades) * 100.0 if trades else 0.0,
        "total_pnl": total_pnl,
        "avg_trade_pnl": total_pnl / len(trades) if trades else 0.0,
        "max_drawdown": max_dd,
        "daily_sharpe": daily_sharpe,
        "trade_sharpe": trade_sharpe,
    }


def max_drawdown(equity: np.ndarray) -> float:
    if len(equity) == 0:
        return 0.0
    peaks = np.maximum.accumulate(np.insert(equity, 0, 0.0))[1:]
    return float(np.max(peaks - equity))


def sharpe(values: np.ndarray, periods: float) -> float:
    if len(values) < 2:
        return 0.0
    std = float(np.std(values, ddof=1))
    if std <= 1e-12:
        return 0.0
    return float(np.mean(values) / std * periods)


def stage_one(closes_by_tf: dict[str, pd.DataFrame]) -> pd.DataFrame:
    rows = []
    for timeframe, spec in GRIDS.items():
        closes = closes_by_tf[timeframe]
        print(f"Stage 1 timeframe {timeframe}", flush=True)
        for pair_id, a, b, beta in PAIRS:
            print(f"  sweeping {pair_id}", flush=True)
            for window, entry, exit_z, hold in itertools.product(
                spec["windows"], ENTRIES, EXITS, spec["holds"]
            ):
                if exit_z >= entry:
                    continue
                params = Params(
                    timeframe=timeframe,
                    pair_id=pair_id,
                    a=a,
                    b=b,
                    beta=beta,
                    window=window,
                    min_samples=max(3, round(window * 0.75)),
                    entry=entry,
                    exit=exit_z,
                    hold=hold,
                )
                rows.append(simulate_pair(closes, params))
    df = pd.DataFrame(rows)
    df.to_csv(OUT_DIR / "stage1_all.csv", index=False)
    return df


def stage_two(closes_by_tf: dict[str, pd.DataFrame], stage1: pd.DataFrame) -> pd.DataFrame:
    candidates = []
    for (_, pair_id), group in stage1.groupby(["timeframe", "pair_id"]):
        candidates.append(group.nlargest(TOP_STAGE1_PER_PAIR_TF, "total_pnl"))
    top = pd.concat(candidates, ignore_index=True)
    rows = []
    seen = set()
    print("Stage 2 beta refinement", flush=True)
    for row in top.itertuples(index=False):
        base = next(pair for pair in PAIRS if pair[0] == row.pair_id)
        for multiplier in BETA_MULTIPLIERS:
            beta = round(base[3] * multiplier, 6)
            key = (
                row.timeframe,
                row.pair_id,
                beta,
                int(row.window),
                float(row.entry),
                float(row.exit),
                int(row.hold),
            )
            if key in seen:
                continue
            seen.add(key)
            params = Params(
                timeframe=row.timeframe,
                pair_id=row.pair_id,
                a=row.a,
                b=row.b,
                beta=beta,
                window=int(row.window),
                min_samples=int(row.min_samples),
                entry=float(row.entry),
                exit=float(row.exit),
                hold=int(row.hold),
            )
            rows.append(simulate_pair(closes_by_tf[row.timeframe], params))
    df = pd.DataFrame(rows)
    df.to_csv(OUT_DIR / "stage2_beta_refine.csv", index=False)
    return df


def write_best_outputs(all_results: pd.DataFrame) -> pd.DataFrame:
    all_results.to_csv(OUT_DIR / "all_results.csv", index=False)
    best_by_pair = (
        all_results.sort_values("total_pnl", ascending=False)
        .groupby("pair_id", as_index=False)
        .head(1)
        .sort_values("total_pnl", ascending=False)
    )
    best_by_pair.to_csv(OUT_DIR / "best_by_pair.csv", index=False)

    top_overall = all_results.sort_values("total_pnl", ascending=False).head(100)
    top_overall.to_csv(OUT_DIR / "top_100_configs.csv", index=False)

    positive = best_by_pair[best_by_pair["total_pnl"] > 0]
    portfolio = portfolio_summary(positive)
    with (OUT_DIR / "portfolio_summary.csv").open("w", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=list(portfolio.keys()))
        writer.writeheader()
        writer.writerow(portfolio)
    return best_by_pair


def portfolio_summary(best_by_pair: pd.DataFrame) -> dict:
    return {
        "pairs": len(best_by_pair),
        "trades": int(best_by_pair["trades"].sum()),
        "wins": int(best_by_pair["wins"].sum()),
        "win_rate": best_by_pair["wins"].sum() / best_by_pair["trades"].sum() * 100.0
        if best_by_pair["trades"].sum()
        else 0.0,
        "total_pnl": float(best_by_pair["total_pnl"].sum()),
        "avg_trade_pnl": float(best_by_pair["total_pnl"].sum() / best_by_pair["trades"].sum())
        if best_by_pair["trades"].sum()
        else 0.0,
        "sum_max_drawdown": float(best_by_pair["max_drawdown"].sum()),
        "avg_daily_sharpe": float(best_by_pair["daily_sharpe"].mean()) if len(best_by_pair) else 0.0,
    }


def verify_with_rust(best_by_pair: pd.DataFrame, limit: int = 20) -> None:
    rows = []
    verify_dir = OUT_DIR / "rust_verify"
    verify_dir.mkdir(exist_ok=True)
    for row in best_by_pair.sort_values("total_pnl", ascending=False).head(limit).itertuples(index=False):
        trades_csv = verify_dir / f"{row.pair_id}_{row.timeframe}_trades.csv"
        cmd = [
            str(ENGINE),
            "backtest-csv",
            "--bars-csv",
            str(GRIDS[row.timeframe]["csv"]),
            "--pair-id",
            row.pair_id,
            "--a",
            row.a,
            "--b",
            row.b,
            "--beta",
            f"{row.beta:.6f}",
            "--leg-notional",
            f"{LEG_NOTIONAL:.2f}",
            "--window",
            str(int(row.window)),
            "--min-samples",
            str(int(row.min_samples)),
            "--entry-zscore",
            f"{row.entry:.2f}",
            "--exit-zscore",
            f"{row.exit:.2f}",
            "--max-holding-bars",
            str(int(row.hold)),
            "--slippage-bps",
            f"{SLIPPAGE_BPS:.2f}",
            "--trades-csv",
            str(trades_csv),
        ]
        result = subprocess.run(cmd, cwd=ROOT, text=True, capture_output=True, check=True)
        (verify_dir / f"{row.pair_id}_{row.timeframe}.txt").write_text(result.stdout)
        rows.append(
            {
                "pair_id": row.pair_id,
                "timeframe": row.timeframe,
                "python_total_pnl": row.total_pnl,
                "rust_total_pnl": grab_report_value(result.stdout, "total_pnl"),
                "python_trades": row.trades,
                "rust_trades": grab_report_value(result.stdout, "trades"),
                "report": str(verify_dir / f"{row.pair_id}_{row.timeframe}.txt"),
                "trades_csv": str(trades_csv),
            }
        )
    pd.DataFrame(rows).to_csv(OUT_DIR / "rust_verification.csv", index=False)


def grab_report_value(text: str, label: str) -> str:
    prefix = f"{label}: "
    for line in text.splitlines():
        if line.startswith(prefix):
            return line[len(prefix) :]
    return ""


def main() -> None:
    ensure_resampled_files()
    closes_by_tf = {name: load_close_matrix(spec["csv"]) for name, spec in GRIDS.items()}
    print("Loaded data:", flush=True)
    for name, closes in closes_by_tf.items():
        print(f"  {name}: {len(closes):5d} bars, {len(closes.columns)} symbols", flush=True)

    stage1 = stage_one(closes_by_tf)
    print(f"Stage 1 configs: {len(stage1)}", flush=True)
    stage2 = stage_two(closes_by_tf, stage1)
    print(f"Stage 2 configs: {len(stage2)}", flush=True)
    all_results = pd.concat([stage1, stage2], ignore_index=True)
    best_by_pair = write_best_outputs(all_results)
    verify_with_rust(best_by_pair)

    print("\nBest by pair:")
    for row in best_by_pair.itertuples(index=False):
        print(
            f"{row.pair_id:8s} {row.timeframe:2s} beta={row.beta:6.3f} "
            f"w={int(row.window):3d} e={row.entry:4.2f} x={row.exit:4.2f} h={int(row.hold):3d} "
            f"trades={int(row.trades):3d} pnl={row.total_pnl:9.2f} "
            f"dd={row.max_drawdown:8.2f} sharpe={row.daily_sharpe:6.2f}"
        )
    print(f"\nWrote outputs to {OUT_DIR}")


if __name__ == "__main__":
    main()
