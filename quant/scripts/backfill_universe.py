"""历史成分股回填:用 baostock 历史成分查询重建 quant_index_member 区间。

解决幸存者偏差:名录表此前只有首次同步日之后的变动,pool_at(历史日期)
拿不到真实历史成分。本脚本按 --step-days 间隔采样历史成分,重建
in_date/out_date 区间(粒度误差 <= 采样间隔)。

运行: cd quant && uv run python scripts/backfill_universe.py [--start 2019-01-01]
"""
from __future__ import annotations

import argparse
import sys
from datetime import date
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.data import baostock_client, universe
from app.db import SessionLocal


def main() -> None:
    ap = argparse.ArgumentParser(description="重建指数历史成分区间")
    ap.add_argument("--start", default="2019-01-01",
                    help="采样起点 YYYY-MM-DD(默认 2019-01-01)")
    ap.add_argument("--step-days", type=int, default=14,
                    help="采样间隔天数(默认 14,越小越精确、越慢)")
    args = ap.parse_args()
    start = date.fromisoformat(args.start)

    with SessionLocal() as db, baostock_client.login_session():
        for name in universe.INDEX_NAMES:
            r = universe.rebuild_index_members(db, name, start,
                                               step_days=args.step_days)
            print(name, r)


if __name__ == "__main__":
    main()
