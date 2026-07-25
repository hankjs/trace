"""将用户隔离升级前的共享自选、成交和回测显式归属给指定用户。

运行: uv run python scripts/claim_legacy_user_data.py --user-id <UUID>
用户 id 是共享 users.id 的 VARCHAR(36) UUID 字符串。
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from sqlalchemy import select, update  # noqa: E402

from app.db import SessionLocal  # noqa: E402
from app.models import BacktestRun, Stock, Trade, WatchlistItem  # noqa: E402


def main() -> None:
    parser = argparse.ArgumentParser(description="认领量化系统升级前的共享用户数据")
    parser.add_argument("--user-id", required=True, type=str)
    args = parser.parse_args()
    user_id = args.user_id.strip()
    if not user_id:
        parser.error("--user-id 不能为空")

    with SessionLocal() as db:
        trades = db.execute(
            update(Trade).where(Trade.user_id.is_(None)).values(user_id=user_id)
        ).rowcount or 0
        runs = db.execute(
            update(BacktestRun).where(BacktestRun.user_id.is_(None))
            .values(user_id=user_id)
        ).rowcount or 0
        legacy_codes = db.execute(
            select(Stock.code).where(Stock.is_watch.is_(True))
        ).scalars().all()
        watches = 0
        for code in legacy_codes:
            if db.get(WatchlistItem, (user_id, code)) is None:
                db.add(WatchlistItem(user_id=user_id, code=code))
                watches += 1
        db.commit()
    print(f"已认领: 成交 {trades} 条, 回测 {runs} 条, 自选 {watches} 只")


if __name__ == "__main__":
    main()
