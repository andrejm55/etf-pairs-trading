#!/usr/bin/env python3
"""Optimize the conservative pair set on minute-derived RTH bars.

The split is intentionally simple and out-of-sample:
- train: 2025-05-12 through 2025-12-31
- test:  2026-01-01 through 2026-05-12

The final candidates should be verified with the Rust `backtest-csv` command.
"""

from __future__ import annotations

import itertools
import math
from dataclasses import asdict, dataclass
from pathlib import Path

import pandas as pd


ROOT = Path(__file__).resolve().parents[2]
DATA_DIR = ROOT / "data" / "minute_rth_aggregated_2025-05-12_2026-05-12"
OUT_DIR = ROOT / "tmp_backtests" / "conservative_rth_optimization"

LEG_NOTIONAL = 2000.0
SLIPPAGE_BPS = 1.0
TRAIN_END = pd.Timestamp("2025-12-31 23:59:59", tz="UTC")
TEST_START = pd.Timestamp("2026-01-01 00:00:00", tz="UTC")

PAIRS = [
    ("SPY_IWM", "SPY", "IWM", 3.5100),
    ("QQQ_XLK", "QQQ", "XLK", 3.7050),
    ("DIA_SPY", "DIA", "SPY", 0.8500),
    ("SPY_XLK", "SPY", "XLK", 2.2425),
]

TIMEFRAMES = {
    "1h": DATA_DIR / "tested_etfs_1hour_rth_from_1min.csv",
    "4h": DATA_DIR / "tested_etfs_4hour_rth_from_1min.csv",
    "1d": DATA_DIR / "tested_etfs_1day_rth_from_1min.csv",
}

WINDOWS = {
    "1h": [60, 195],
    "4h": [30, 98],
    "1d": [20, 60],
}
HOLDS = {
    "1h": [60, 195],
    "4h": [24, 98],
    "1d": [10, 40, 90],
}
ENTRIES = [1.25, 1.50, 1.75]
EXITS = [0.00, 0.50]
BETA_MULTIPLIERS = [0.85, 1.00, 1.15]
MIN_MOVES = [0.0, 25.0, 50.0]
ADVERSE_ZS = [0.0, 1.00, 1.50]
STOP_LOSSES = [0.0, 100.0, 150.0]
TOP_N = 20


@dataclass(frozen=True)
class Params:
    pair: str
    a: str
    b: str
    timeframe: str
    beta: float
    window: int
    min_samples: int
    entry: float
    exit: float
    hold: int
    min_move: float
    adverse_z: float
    stop_loss: float


@dataclass
class Result:
    trades: int
    wins: int
    pnl: float
    avg: float
    max_drawdown: float
    score: float


def load_close_matrix(path: Path) -> pd.DataFrame:
    df = pd.read_csv(path, parse_dates=["t"], usecols=["symbol", "t", "c"])
    return df.pivot(index="t", columns="symbol", values="c").sort_index()


def balanced_quantities(target: float, price_a: float, price_b: float) -> tuple[int, int]:
    return max(1, round(target / price_a)), max(1, round(target / price_b))


def fill_price(mid: float, side: str) -> float:
    slip = SLIPPAGE_BPS / 10_000.0
    return mid * (1.0 + slip) if side == "buy" else mid * (1.0 - slip)


def leg_pnl(side: str, entry: float, exit_price: float, qty: int) -> float:
    return (exit_price - entry) * qty if side == "buy" else (entry - exit_price) * qty


def close_pnl(pos: dict, a_mid: float, b_mid: float) -> float:
    exit_a = fill_price(a_mid, "sell" if pos["side_a"] == "buy" else "buy")
    exit_b = fill_price(b_mid, "sell" if pos["side_b"] == "buy" else "buy")
    return leg_pnl(pos["side_a"], pos["entry_a"], exit_a, pos["qty_a"]) + leg_pnl(
        pos["side_b"], pos["entry_b"], exit_b, pos["qty_b"]
    )


def max_drawdown(curve: list[float]) -> float:
    peak = 0.0
    worst = 0.0
    for value in curve:
        peak = max(peak, value)
        worst = max(worst, peak - value)
    return worst


def simulate(closes: pd.DataFrame, params: Params) -> Result:
    data = closes[[params.a, params.b]].dropna()
    if len(data) < params.min_samples + 2:
        return Result(0, 0, 0.0, 0.0, 0.0, -1_000_000.0)

    a_prices = data[params.a].to_numpy(dtype=float)
    b_prices = data[params.b].to_numpy(dtype=float)
    spread = pd.Series(a_prices - params.beta * b_prices, index=data.index)
    mean = spread.rolling(params.window, min_periods=params.min_samples).mean()
    std = spread.rolling(params.window, min_periods=params.min_samples).std(ddof=1).clip(lower=1e-9)
    z = ((spread - mean) / std).to_numpy(dtype=float)
    std_values = std.to_numpy(dtype=float)

    pos = None
    trades = []
    realised = 0.0
    curve = []

    for i, z_value in enumerate(z):
        if math.isnan(z_value):
            continue
        a_mid = a_prices[i]
        b_mid = b_prices[i]

        if pos is not None:
            hold = i - pos["open_i"]
            pnl_now = close_pnl(pos, a_mid, b_mid)
            entry_side = math.copysign(1.0, pos["entry_z"])
            adverse_move = entry_side * (z_value - pos["entry_z"])
            stop_hit = params.stop_loss > 0 and pnl_now <= -params.stop_loss
            adverse_hit = params.adverse_z > 0 and adverse_move >= params.adverse_z
            converged = abs(z_value) <= params.exit
            timed_out = hold >= params.hold
            if stop_hit or adverse_hit or converged or timed_out:
                trades.append(pnl_now)
                realised += pnl_now
                curve.append(realised)
                pos = None
                continue

        if pos is None:
            if abs(z_value) < params.entry:
                continue
            expected_move = ((abs(z_value) - params.exit) * std_values[i]) * (
                LEG_NOTIONAL / max(a_mid, 1e-9)
            )
            if expected_move < params.min_move:
                continue
            side_a, side_b = ("sell", "buy") if z_value > 0 else ("buy", "sell")
            qty_a, qty_b = balanced_quantities(LEG_NOTIONAL, a_mid, b_mid)
            pos = {
                "open_i": i,
                "entry_z": z_value,
                "side_a": side_a,
                "side_b": side_b,
                "qty_a": qty_a,
                "qty_b": qty_b,
                "entry_a": fill_price(a_mid, side_a),
                "entry_b": fill_price(b_mid, side_b),
            }

    if pos is not None:
        pnl_now = close_pnl(pos, a_prices[-1], b_prices[-1])
        trades.append(pnl_now)
        realised += pnl_now
        curve.append(realised)

    wins = sum(1 for pnl in trades if pnl > 0)
    dd = max_drawdown(curve)
    avg = realised / len(trades) if trades else 0.0
    missing_trade_penalty = max(0, 3 - len(trades)) * 100.0
    score = realised - dd - missing_trade_penalty
    return Result(len(trades), wins, realised, avg, dd, score)


def candidate_params(pair: tuple[str, str, str, float], timeframe: str) -> list[Params]:
    pair_id, a, b, base_beta = pair
    params = []
    for beta_mult, window, hold, entry, exit_z, min_move, adverse_z, stop_loss in itertools.product(
        BETA_MULTIPLIERS,
        WINDOWS[timeframe],
        HOLDS[timeframe],
        ENTRIES,
        EXITS,
        MIN_MOVES,
        ADVERSE_ZS,
        STOP_LOSSES,
    ):
        if exit_z >= entry:
            continue
        min_samples = max(5, round(window * 0.75))
        params.append(
            Params(
                pair=pair_id,
                a=a,
                b=b,
                timeframe=timeframe,
                beta=round(base_beta * beta_mult, 4),
                window=window,
                min_samples=min_samples,
                entry=entry,
                exit=exit_z,
                hold=hold,
                min_move=min_move,
                adverse_z=adverse_z,
                stop_loss=stop_loss,
            )
        )
    return params


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    matrices = {tf: load_close_matrix(path) for tf, path in TIMEFRAMES.items()}
    train = {tf: matrix[matrix.index <= TRAIN_END] for tf, matrix in matrices.items()}
    test = {tf: matrix[matrix.index >= TEST_START] for tf, matrix in matrices.items()}

    all_rows = []
    best_rows = []
    for pair in PAIRS:
        pair_rows = []
        for timeframe in TIMEFRAMES:
            closes = train[timeframe]
            for params in candidate_params(pair, timeframe):
                result = simulate(closes, params)
                row = {**asdict(params), **{f"train_{k}": v for k, v in asdict(result).items()}}
                pair_rows.append(row)
        pair_df = pd.DataFrame(pair_rows).sort_values("train_score", ascending=False)
        pair_df.to_csv(OUT_DIR / f"{pair[0]}_train_sweep.csv", index=False)

        top = pair_df.head(TOP_N).copy()
        test_rows = []
        for row in top.to_dict("records"):
            params = Params(
                pair=row["pair"],
                a=row["a"],
                b=row["b"],
                timeframe=row["timeframe"],
                beta=row["beta"],
                window=int(row["window"]),
                min_samples=int(row["min_samples"]),
                entry=row["entry"],
                exit=row["exit"],
                hold=int(row["hold"]),
                min_move=row["min_move"],
                adverse_z=row["adverse_z"],
                stop_loss=row["stop_loss"],
            )
            test_result = simulate(test[params.timeframe], params)
            test_rows.append({**row, **{f"test_{k}": v for k, v in asdict(test_result).items()}})
        tested = pd.DataFrame(test_rows).sort_values("test_pnl", ascending=False)
        tested.to_csv(OUT_DIR / f"{pair[0]}_top_train_test.csv", index=False)
        all_rows.extend(tested.to_dict("records"))
        best_rows.append(tested.iloc[0].to_dict())

    all_df = pd.DataFrame(all_rows)
    best_df = pd.DataFrame(best_rows).sort_values("test_pnl", ascending=False)
    all_df.to_csv(OUT_DIR / "top_train_configs_tested.csv", index=False)
    best_df.to_csv(OUT_DIR / "best_by_pair_walk_forward.csv", index=False)
    print(best_df[
        [
            "pair",
            "timeframe",
            "beta",
            "window",
            "entry",
            "exit",
            "hold",
            "min_move",
            "adverse_z",
            "stop_loss",
            "train_trades",
            "train_pnl",
            "test_trades",
            "test_pnl",
            "test_max_drawdown",
        ]
    ].to_string(index=False))
    print(f"\\nOutput: {OUT_DIR}")


if __name__ == "__main__":
    main()
