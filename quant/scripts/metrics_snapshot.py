"""修复前后指标对照采集脚本(只读生产库,不落库)。

固定策略/标的/区间跑 run_backtest(save=False),把指标 dump 成 JSON。
先在修复前跑一次存 before.json,修完再跑存 after.json,产出
logs/metrics-before-after.md 的数据来源。

运行:
  cd quant && uv run python scripts/metrics_snapshot.py logs/_metrics_before.json
"""
from __future__ import annotations

import json
import sys
from datetime import date
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.backtest.engine import run_backtest  # noqa: E402
from app.db import SessionLocal  # noqa: E402

# 固定样本:流动性好、数据完整的大盘股,避开停牌/退市噪声
CODES = ["sh.600519", "sz.000001", "sh.600036", "sh.601318", "sz.000002"]
START = date(2023, 1, 1)
END = date(2025, 12, 31)
CASES = [
    ("ma_cross", CODES, None),
    ("breakout", CODES, None),
    ("mean_reversion", CODES, None),
    ("volume_breakout", CODES, None),
    ("momentum_rotation", CODES, None),
    ("multifactor_hold", CODES, None),
]


def main() -> None:
    out_path = Path(sys.argv[1] if len(sys.argv) > 1 else "logs/_metrics.json")
    snapshot: dict = {"start": str(START), "end": str(END), "codes": CODES,
                      "cases": {}}
    with SessionLocal() as db:
        for strategy, codes, params in CASES:
            try:
                res = run_backtest(db, strategy, codes, START, END,
                                   params=params, save=False)
                m = dict(res["metrics"])
                per = m.pop("per_code", None)
                snapshot["cases"][strategy] = {
                    "metrics": m,
                    "per_code": {c: v for c, v in (per or {}).items()},
                    "sample": res["codes"],
                }
                print(f"OK   {strategy}: {m}")
            except Exception as exc:  # noqa: BLE001
                snapshot["cases"][strategy] = {"error": str(exc)}
                print(f"FAIL {strategy}: {exc}")
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(snapshot, ensure_ascii=False, indent=2))
    print(f"\nwrote {out_path}")


if __name__ == "__main__":
    main()
