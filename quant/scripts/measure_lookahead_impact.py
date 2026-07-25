"""P0(提前一天建仓)在真实数据上的影响量化。

做法:对每个单标的策略、每只样本股,逐个候选起点找出"起点当日恰好由 0 翻 1"
的日子——那正是旧实现会用当日开盘价成交当日收盘信号的场合。对这些起点
分别用旧逻辑与新逻辑各跑一次,输出 total_return 差异。

旧逻辑用 monkeypatch 还原(判断窗口首日仓位水平),不改动生产代码。

运行: cd quant && uv run python scripts/measure_lookahead_impact.py
"""
from __future__ import annotations

import json
import sys
from datetime import date, timedelta
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import pandas as pd  # noqa: E402

from app.backtest import engine  # noqa: E402
from app.data.ingest import load_bars_df  # noqa: E402
from app.db import SessionLocal  # noqa: E402
from app.strategy.strategies import REGISTRY, SINGLE_TEMPLATES  # noqa: E402

CODES = ["sh.600519", "sz.000001", "sh.600036", "sh.601318", "sz.000002",
         "sz.300750", "sh.688111", "sz.002594"]
END = date(2025, 12, 31)
WINDOW_DAYS = 400
MAX_CASES_PER_STRATEGY = 6


def _old_held_before(pos: pd.Series, first_bar: pd.Timestamp) -> bool:
    """修复前的判定:看窗口首日的仓位水平(而非起点前一根)。"""
    if first_bar not in pos.index:
        return False
    return float(pos.loc[first_bar]) == 1.0


def main() -> None:
    cases: list[dict] = []
    with SessionLocal() as db:
        for strategy in SINGLE_TEMPLATES:
            mod = REGISTRY[strategy]
            found = 0
            for code in CODES:
                if found >= MAX_CASES_PER_STRATEGY:
                    break
                df = load_bars_df(db, code, start=END - timedelta(days=WINDOW_DAYS),
                                  end=END)
                if len(df) < 120:
                    continue
                pos = mod.positions(df, None)
                arr = pos.astype(float).to_numpy()
                # 找 0 -> 1 的翻仓点,且其后仍有足够 bar 供回测
                for i in range(1, len(arr) - engine.MIN_BARS - 5):
                    if not (arr[i] == 1.0 and arr[i - 1] == 0.0):
                        continue
                    bt_start = df["date"].iloc[i]
                    try:
                        new = engine._batch_single(
                            {code: df}, {code: pos}, engine.DEFAULT_COSTS, bt_start)
                        engine._held_before = _old_held_before
                        old = engine._batch_single(
                            {code: df}, {code: pos}, engine.DEFAULT_COSTS, bt_start)
                    finally:
                        engine._held_before = _HELD_BEFORE_ORIG
                    if code not in new or code not in old:
                        continue
                    nm, om = new[code]["metrics"], old[code]["metrics"]
                    if abs(nm["total_return"] - om["total_return"]) < 1e-9:
                        continue
                    cases.append({
                        "strategy": strategy, "code": code,
                        "start": str(bt_start),
                        "before_total_return": om["total_return"],
                        "after_total_return": nm["total_return"],
                        "delta": round(nm["total_return"] - om["total_return"], 4),
                        "before_trades": om["trade_count"],
                        "after_trades": nm["trade_count"],
                    })
                    found += 1
                    break

    Path("logs/_lookahead_cases.json").write_text(
        json.dumps(cases, ensure_ascii=False, indent=2))
    print(f"命中 {len(cases)} 例(起点当日恰好翻仓):")
    for c in cases:
        print(f"  {c['strategy']:18s} {c['code']} start={c['start']} "
              f"before={c['before_total_return']:+.4f} "
              f"after={c['after_total_return']:+.4f} "
              f"delta={c['delta']:+.4f} "
              f"trades {c['before_trades']}->{c['after_trades']}")
    if cases:
        deltas = [c["delta"] for c in cases]
        print(f"\ndelta: min={min(deltas):+.4f} max={max(deltas):+.4f} "
              f"mean={sum(deltas)/len(deltas):+.4f}")
        inflated = [c for c in cases if c["delta"] < 0]
        print(f"修复后收益下降(即旧口径虚增)的占比: "
              f"{len(inflated)}/{len(cases)}")


_HELD_BEFORE_ORIG = engine._held_before

if __name__ == "__main__":
    main()
