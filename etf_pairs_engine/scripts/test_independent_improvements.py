#!/usr/bin/env python3
"""Independently test four research improvements against the six-pair baseline.

This is intentionally a research script. It does not change the live engine.
"""

from __future__ import annotations

import csv
import math
from collections import defaultdict, deque
from dataclasses import dataclass
from datetime import timedelta
from pathlib import Path

import pandas as pd


ROOT = Path(__file__).resolve().parents[2]
BASELINE_TRADES = ROOT / "tmp_backtests" / "all_pairs_enhanced_alpaca_4h_2022_2026" / "all_trades.csv"
BARS_CSV = ROOT / "data" / "tested_etfs_alpaca_4hour_2022-01-01_2026-07-06.csv"
OUT_DIR = ROOT / "tmp_backtests" / "independent_improvement_tests"

LEG_NOTIONAL = 2000.0
SLIPPAGE_BPS = 1.0
SELECTED_PAIRS = ["XLE_OIH", "SPY_IWM", "SPY_QQQ", "QQQ_XLK", "DIA_SPY", "EFA_EEM"]


@dataclass(frozen=True)
class PairSpec:
    pair_id: str
    a: str
    b: str
    beta: float
    template: str


@dataclass(frozen=True)
class Template:
    window: int
    min_samples: int
    entry: float
    exit: float
    hold: int
    use_rolling_beta: bool = False
    beta_min_mult: float | None = None
    beta_max_mult: float | None = None
    min_corr: float | None = None
    entry_confirmation: int = 0
    min_expected_move: float | None = None
    adverse_z: float | None = None
    stop_loss: float | None = None


PAIR_SPECS = {
    "XLE_OIH": PairSpec("XLE_OIH", "XLE", "OIH", 1.6100, "confirmation_corr_4h"),
    "SPY_IWM": PairSpec("SPY_IWM", "SPY", "IWM", 3.5100, "confirmation_corr_4h"),
    "SPY_QQQ": PairSpec("SPY_QQQ", "SPY", "QQQ", 0.78, "rolling_beta_4h"),
    "QQQ_XLK": PairSpec("QQQ_XLK", "QQQ", "XLK", 3.1492, "rolling_beta_4h"),
    "DIA_SPY": PairSpec("DIA_SPY", "DIA", "SPY", 0.9775, "confirmation_corr_4h"),
    "EFA_EEM": PairSpec("EFA_EEM", "EFA", "EEM", 1.80, "rolling_beta_4h"),
}

TEMPLATES = {
    "rolling_beta_4h": Template(
        window=60,
        min_samples=45,
        entry=1.25,
        exit=0.50,
        hold=98,
        use_rolling_beta=True,
        beta_min_mult=0.75,
        beta_max_mult=1.25,
        adverse_z=1.50,
        stop_loss=100.0,
    ),
    "confirmation_corr_4h": Template(
        window=30,
        min_samples=22,
        entry=1.50,
        exit=0.00,
        hold=48,
        min_corr=0.50,
        entry_confirmation=1,
        min_expected_move=50.0,
        adverse_z=0.0,
        stop_loss=100.0,
    ),
}


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


def fill_price(mid: float, side: str) -> float:
    slip = SLIPPAGE_BPS / 10_000.0
    return mid * (1.0 + slip) if side == "buy" else mid * (1.0 - slip)


def leg_pnl(side: str, entry: float, exit_price: float, qty: int) -> float:
    if side == "buy":
        return (exit_price - entry) * qty
    return (entry - exit_price) * qty


def max_drawdown(values: list[float]) -> float:
    peak = 0.0
    out = 0.0
    for value in values:
        peak = max(peak, value)
        out = max(out, peak - value)
    return out


def summarize(trades: pd.DataFrame) -> dict:
    if trades.empty:
        return {"trades": 0, "wins": 0, "win_rate": 0.0, "pnl": 0.0, "avg": 0.0, "max_dd": 0.0, "pnl_dd": 0.0}
    ordered = trades.sort_values("closed_at").copy()
    equity = ordered["pnl"].cumsum().tolist()
    pnl = float(ordered["pnl"].sum())
    dd = max_drawdown(equity)
    wins = int((ordered["pnl"] > 0).sum())
    count = len(ordered)
    return {
        "trades": count,
        "wins": wins,
        "win_rate": wins / count * 100.0,
        "pnl": pnl,
        "avg": pnl / count,
        "max_dd": dd,
        "pnl_dd": pnl / dd if dd > 0 else float("inf"),
    }


def load_baseline_trades() -> pd.DataFrame:
    df = pd.read_csv(BASELINE_TRADES, parse_dates=["opened_at", "closed_at"])
    return df[df["pair_id"].isin(SELECTED_PAIRS)].copy()


def load_close_matrix() -> pd.DataFrame:
    raw = pd.read_csv(BARS_CSV, parse_dates=["t"], usecols=["symbol", "t", "c"])
    return raw.pivot(index="t", columns="symbol", values="c").sort_index()


def mean(values: deque[float]) -> float:
    return sum(values) / len(values)


def stddev(values: deque[float], avg: float) -> float:
    denom = max(len(values) - 1, 1)
    return math.sqrt(sum((x - avg) ** 2 for x in values) / denom)


def rolling_beta(a_prices: deque[float], b_prices: deque[float]) -> float | None:
    n = min(len(a_prices), len(b_prices))
    if n < 3:
        return None
    aa = list(reversed(a_prices))[:n]
    bb = list(reversed(b_prices))[:n]
    ma = sum(aa) / n
    mb = sum(bb) / n
    cov = sum((aa[i] - ma) * (bb[i] - mb) for i in range(n))
    vb = sum((bb[i] - mb) ** 2 for i in range(n))
    return None if abs(vb) < 1e-12 else cov / vb


def correlation(a_returns: deque[float], b_returns: deque[float]) -> float:
    n = min(len(a_returns), len(b_returns))
    if n < 3:
        return 0.0
    aa = list(reversed(a_returns))[:n]
    bb = list(reversed(b_returns))[:n]
    ma = sum(aa) / n
    mb = sum(bb) / n
    cov = sum((aa[i] - ma) * (bb[i] - mb) for i in range(n))
    va = sum((aa[i] - ma) ** 2 for i in range(n))
    vb = sum((bb[i] - mb) ** 2 for i in range(n))
    return cov / max(math.sqrt(va) * math.sqrt(vb), 1e-12)


def unrealized_pnl(pos: dict, a_mid: float, b_mid: float) -> float:
    exit_a = fill_price(a_mid, "sell" if pos["side_a"] == "buy" else "buy")
    exit_b = fill_price(b_mid, "sell" if pos["side_b"] == "buy" else "buy")
    return leg_pnl(pos["side_a"], pos["entry_a"], exit_a, pos["qty_a"]) + leg_pnl(
        pos["side_b"], pos["entry_b"], exit_b, pos["qty_b"]
    )


def close_trade(pos: dict, ts, i: int, z_value: float, a_mid: float, b_mid: float, reason: str) -> dict:
    return {
        "pair_id": pos["pair_id"],
        "opened_at": pos["opened_at"],
        "closed_at": ts,
        "holding_bars": i - pos["open_i"],
        "qty_a": pos["qty_a"],
        "qty_b": pos["qty_b"],
        "entry_z": pos["entry_z"],
        "exit_z": z_value,
        "pnl": unrealized_pnl(pos, a_mid, b_mid),
        "reason": reason,
    }


def simulate_pair(closes: pd.DataFrame, spec: PairSpec, exit_variant: str = "baseline") -> pd.DataFrame:
    tmpl = TEMPLATES[spec.template]
    data = closes[[spec.a, spec.b]].dropna()
    spreads: deque[float] = deque()
    a_prices: deque[float] = deque()
    b_prices: deque[float] = deque()
    a_returns: deque[float] = deque()
    b_returns: deque[float] = deque()
    last_a = None
    last_b = None
    pending_action = None
    pending_extreme = 0.0
    pending_confirmations = 0
    pos = None
    trades = []

    for i, (ts, row) in enumerate(data.iterrows()):
        a_mid = float(row[spec.a])
        b_mid = float(row[spec.b])
        a_prices.append(a_mid)
        b_prices.append(b_mid)
        while len(a_prices) > tmpl.window:
            a_prices.popleft()
            b_prices.popleft()
        if last_a is not None and last_b is not None:
            a_returns.append(math.log(a_mid / last_a))
            b_returns.append(math.log(b_mid / last_b))
            while len(a_returns) > tmpl.window:
                a_returns.popleft()
                b_returns.popleft()
        last_a, last_b = a_mid, b_mid

        beta = spec.beta
        if tmpl.use_rolling_beta and len(a_prices) >= tmpl.min_samples:
            beta = rolling_beta(a_prices, b_prices) or spec.beta
            beta = max(spec.beta * (tmpl.beta_min_mult or 0.0), min(spec.beta * (tmpl.beta_max_mult or 999.0), beta))

        spread = a_mid - beta * b_mid
        spreads.append(spread)
        while len(spreads) > tmpl.window:
            spreads.popleft()
        if len(spreads) < tmpl.min_samples:
            continue
        avg = mean(spreads)
        std = max(stddev(spreads, avg), 1e-9)
        z_value = (spread - avg) / std
        corr = correlation(a_returns, b_returns)

        if pos is not None:
            hold = i - pos["open_i"]
            pnl_now = unrealized_pnl(pos, a_mid, b_mid)
            pos["max_open_pnl"] = max(pos["max_open_pnl"], pnl_now)
            base_exit = abs(z_value) <= tmpl.exit
            adverse = False
            if tmpl.adverse_z is not None:
                entry_side = math.copysign(1.0, pos["entry_z"])
                adverse = entry_side * (z_value - pos["entry_z"]) >= tmpl.adverse_z
            stop = tmpl.stop_loss is not None and pnl_now <= -abs(tmpl.stop_loss)
            max_hold = hold >= tmpl.hold
            time_decay = False
            profit_protect = False
            if exit_variant == "time_decay":
                # If a trade has not converged halfway through its allowed hold and z has barely improved, cut it.
                initial_abs_z = abs(pos["entry_z"])
                time_decay = hold >= max(6, tmpl.hold // 2) and abs(z_value) > max(tmpl.exit, initial_abs_z * 0.75)
            elif exit_variant == "profit_protection":
                # Protect only meaningful open profit; avoid turning decent winners into flat or losing trades.
                profit_protect = pos["max_open_pnl"] >= 50.0 and pnl_now <= max(10.0, pos["max_open_pnl"] * 0.50)
            elif exit_variant == "pair_specific":
                if spec.pair_id in {"SPY_IWM", "EFA_EEM"}:
                    time_decay = hold >= max(6, tmpl.hold // 2) and abs(z_value) > max(tmpl.exit, abs(pos["entry_z"]) * 0.75)
                if spec.pair_id in {"XLE_OIH", "SPY_QQQ", "QQQ_XLK"}:
                    profit_protect = pos["max_open_pnl"] >= 50.0 and pnl_now <= max(10.0, pos["max_open_pnl"] * 0.50)

            if base_exit or adverse or stop or max_hold or time_decay or profit_protect:
                if base_exit:
                    reason = "zscore_converged"
                elif adverse:
                    reason = "adverse_z"
                elif stop:
                    reason = "stop_loss"
                elif time_decay:
                    reason = "time_decay"
                elif profit_protect:
                    reason = "profit_protection"
                else:
                    reason = "max_holding_bars"
                trades.append(close_trade(pos, ts, i, z_value, a_mid, b_mid, reason))
                pos = None
                continue

        if pos is not None:
            continue
        if tmpl.min_corr is not None and corr < tmpl.min_corr:
            pending_action = None
            continue
        expected_move = max(abs(z_value) - tmpl.exit, 0.0) * std * (LEG_NOTIONAL / max(a_mid, 1e-9))
        if tmpl.min_expected_move is not None and expected_move < tmpl.min_expected_move:
            continue
        action = None
        if z_value >= tmpl.entry:
            action = "short_spread"
        elif z_value <= -tmpl.entry:
            action = "long_spread"
        if action is None:
            pending_action = None
            continue
        if tmpl.entry_confirmation:
            if pending_action == action:
                if abs(z_value) < abs(pending_extreme):
                    pending_confirmations += 1
                else:
                    pending_extreme = z_value
                    pending_confirmations = 0
            else:
                pending_action = action
                pending_extreme = z_value
                pending_confirmations = 0
            if pending_confirmations < tmpl.entry_confirmation:
                continue
            pending_action = None
        qty_a, qty_b = balanced_quantities(LEG_NOTIONAL, a_mid, b_mid)
        side_a, side_b = ("sell", "buy") if action == "short_spread" else ("buy", "sell")
        pos = {
            "pair_id": spec.pair_id,
            "open_i": i,
            "opened_at": ts,
            "entry_z": z_value,
            "side_a": side_a,
            "side_b": side_b,
            "qty_a": qty_a,
            "qty_b": qty_b,
            "entry_a": fill_price(a_mid, side_a),
            "entry_b": fill_price(b_mid, side_b),
            "max_open_pnl": 0.0,
        }

    if pos is not None:
        ts = data.index[-1]
        row = data.iloc[-1]
        trades.append(close_trade(pos, ts, len(data) - 1, 0.0, float(row[spec.a]), float(row[spec.b]), "end_of_backtest"))

    return pd.DataFrame(trades)


def add_metric_row(rows: list[dict], experiment: str, detail: str, trades: pd.DataFrame) -> None:
    summary = summarize(trades)
    rows.append({"experiment": experiment, "detail": detail, **summary})


def write_summary(path: Path, rows: list[dict]) -> None:
    df = pd.DataFrame(rows)
    df.to_csv(path, index=False, float_format="%.4f")
    print(df.to_string(index=False, formatters={
        "win_rate": "{:.2f}".format,
        "pnl": "{:.2f}".format,
        "avg": "{:.2f}".format,
        "max_dd": "{:.2f}".format,
        "pnl_dd": "{:.2f}".format,
    }))


def point_1_baskets(trades: pd.DataFrame, rows: list[dict]) -> None:
    pair_order = (
        trades.groupby("pair_id")["pnl"].sum().sort_values(ascending=False).index.tolist()
    )
    for n in [4, 5, 6]:
        basket = pair_order[:n]
        add_metric_row(rows, "1_basket_selection", f"top_{n}: " + " ".join(basket), trades[trades["pair_id"].isin(basket)])


def point_2_exits(closes: pd.DataFrame, rows: list[dict]) -> None:
    variants = ["baseline_resim", "time_decay", "profit_protection", "pair_specific"]
    for variant in variants:
        frames = []
        sim_variant = "baseline" if variant == "baseline_resim" else variant
        for pair_id in SELECTED_PAIRS:
            frames.append(simulate_pair(closes, PAIR_SPECS[pair_id], sim_variant))
        combined = pd.concat(frames, ignore_index=True)
        add_metric_row(rows, "2_exit_logic", variant, combined)


def point_3_vol_sizing(trades: pd.DataFrame, rows: list[dict]) -> None:
    baseline_pair_dd = {}
    for pair_id, group in trades.groupby("pair_id"):
        baseline_pair_dd[pair_id] = summarize(group)["max_dd"]
    target_dd = pd.Series(baseline_pair_dd).median()
    for cap in [1.25, 1.50, 2.00]:
        adjusted = trades.copy()
        scales = {
            pair_id: max(0.50, min(cap, target_dd / max(dd, 1e-9)))
            for pair_id, dd in baseline_pair_dd.items()
        }
        adjusted["pnl"] = adjusted.apply(lambda r: r["pnl"] * scales[r["pair_id"]], axis=1)
        detail = "cap_{:.2f}: ".format(cap) + " ".join(f"{k}={v:.2f}x" for k, v in sorted(scales.items()))
        add_metric_row(rows, "3_vol_adjusted_sizing", detail, adjusted)


def point_4_pair_kill_switches(trades: pd.DataFrame, rows: list[dict]) -> None:
    ordered = trades.sort_values("opened_at").copy()
    variants = [
        ("loss_cooldown_20d", {"losses": 1, "cooldown_days": 20, "rolling_months": None}),
        ("two_loss_cooldown_45d", {"losses": 2, "cooldown_days": 45, "rolling_months": None}),
        ("rolling_3m_negative_skip_30d", {"losses": None, "cooldown_days": 30, "rolling_months": 3}),
    ]
    for name, cfg in variants:
        kept = []
        disabled_until = defaultdict(lambda: pd.Timestamp.min.tz_localize("UTC"))
        consecutive_losses = defaultdict(int)
        realized_by_pair: dict[str, list[tuple[pd.Timestamp, float]]] = defaultdict(list)
        skipped = 0
        for _, trade in ordered.iterrows():
            pair_id = trade["pair_id"]
            opened_at = trade["opened_at"]
            if opened_at < disabled_until[pair_id]:
                skipped += 1
                continue
            if cfg["rolling_months"] is not None:
                cutoff = opened_at - pd.DateOffset(months=cfg["rolling_months"])
                rolling_pnl = sum(pnl for ts, pnl in realized_by_pair[pair_id] if ts >= cutoff)
                rolling_count = sum(1 for ts, _ in realized_by_pair[pair_id] if ts >= cutoff)
                if rolling_count >= 3 and rolling_pnl < 0:
                    disabled_until[pair_id] = opened_at + timedelta(days=cfg["cooldown_days"])
                    skipped += 1
                    continue
            kept.append(trade.to_dict())
            pnl = float(trade["pnl"])
            realized_by_pair[pair_id].append((trade["closed_at"], pnl))
            if pnl < 0:
                consecutive_losses[pair_id] += 1
            else:
                consecutive_losses[pair_id] = 0
            if cfg["losses"] is not None and consecutive_losses[pair_id] >= cfg["losses"]:
                disabled_until[pair_id] = trade["closed_at"] + timedelta(days=cfg["cooldown_days"])
                consecutive_losses[pair_id] = 0
        filtered = pd.DataFrame(kept)
        add_metric_row(rows, "4_pair_kill_switches", f"{name}; skipped={skipped}", filtered)


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    baseline = load_baseline_trades()
    closes = load_close_matrix()
    rows = []
    add_metric_row(rows, "0_current_baseline", "saved six-pair trade stream", baseline)
    point_1_baskets(baseline, rows)
    point_2_exits(closes, rows)
    point_3_vol_sizing(baseline, rows)
    point_4_pair_kill_switches(baseline, rows)
    write_summary(OUT_DIR / "summary.csv", rows)

    pair_year = (
        baseline.assign(year=baseline["closed_at"].dt.year)
        .groupby(["pair_id", "year"])["pnl"]
        .sum()
        .reset_index()
    )
    pair_year.to_csv(OUT_DIR / "baseline_pair_year_pnl.csv", index=False, float_format="%.4f")


if __name__ == "__main__":
    main()
