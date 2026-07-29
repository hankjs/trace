"""历史成分股回填(兼容入口)。

推荐两阶段(拉数与写库分离,与日 K 离线链路一致):

  # 1) 只下载到文件(可在出口机跑;串行 + flock)
  uv run python scripts/download_index_members.py --start 2015-01-01 --estimate
  uv run python scripts/download_index_members.py --start 2015-01-01 --sleep 0.35

  # 2) 本机读文件写库;生产加 --live-sync 对齐当前成分
  uv run python scripts/ingest_index_members_from_files.py --estimate
  uv run python scripts/ingest_index_members_from_files.py --live-sync

本脚本保留「在线一次跑完」模式(边拉边写),仅适合小区间调试。
默认 --start 现为 2015-01-01(库内若仍从 2019 起,用两阶段重灌即可)。
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
    ap = argparse.ArgumentParser(
        description="重建指数历史成分(在线;生产请用 download+ingest 两阶段)",
    )
    ap.add_argument("--start", default="2015-01-01",
                    help="采样起点 YYYY-MM-DD(默认 2015-01-01)")
    ap.add_argument("--step-days", type=int, default=14,
                    help="采样间隔天数(默认 14,越小越精确、越慢)")
    ap.add_argument(
        "--no-live-sync", action="store_true",
        help="重建后不调 baostock 当前成分增量同步",
    )
    args = ap.parse_args()
    start = date.fromisoformat(args.start)

    print(
        "提示: 全量 2015→今 建议改用\n"
        "  scripts/download_index_members.py\n"
        "  scripts/ingest_index_members_from_files.py\n"
        "本命令为在线边拉边写。",
        file=sys.stderr,
    )

    with SessionLocal() as db, baostock_client.login_session():
        for name in universe.INDEX_NAMES:
            r = universe.rebuild_index_members(
                db, name, start,
                step_days=args.step_days,
                live_sync=not args.no_live_sync,
            )
            print(name, r)


if __name__ == "__main__":
    main()
