"""股票池历史回填:池内全部股票 3 年前复权日线。

特性:
- 断点续跑:已有 >=600 条且最近日期在 10 天内的 code 直接跳过;
- 失败重试:单只最多 3 次,间隔递增;单只失败不影响整体;
- baostock 限速友好:全程复用一次登录,每只间隔 sleep。

运行: cd quant && uv run python scripts/backfill_pool.py [--limit N] [--years 3]
"""
from __future__ import annotations

import argparse
import logging
import sys
import time
from datetime import date, timedelta
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from sqlalchemy import func, select  # noqa: E402

from app.data import baostock_client, ingest, universe  # noqa: E402
from app.db import Base, SessionLocal, engine  # noqa: E402
from app import models  # noqa: E402,F401 - 确保建表元数据注册
from app.models import DailyBar  # noqa: E402

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(name)s: %(message)s",
)
logger = logging.getLogger("backfill_pool")

MIN_BARS = 600          # 3 年约 730 个交易日,留些余量
RECENT_DAYS = 10        # 最近数据在此天数内视为已完成
RETRY = 3
SLEEP_PER_CODE = 0.3    # 限速友好


def _done_codes(db, start: date) -> set[str]:
    """已回填完成的 code:3 年窗口内条数 >= MIN_BARS 且最近日期足够新"""
    cutoff = date.today() - timedelta(days=RECENT_DAYS)
    rows = db.execute(
        select(DailyBar.code, func.count(), func.max(DailyBar.date))
        .where(DailyBar.date >= start)
        .group_by(DailyBar.code)
    ).all()
    return {r[0] for r in rows if r[1] >= MIN_BARS and r[2] >= cutoff}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--limit", type=int, default=0, help="只回填前 N 只(调试用)")
    parser.add_argument("--years", type=int, default=3)
    args = parser.parse_args()

    Base.metadata.create_all(engine)
    start = date.today() - timedelta(days=args.years * 365)

    with SessionLocal() as db:
        pool = universe.current_pool(db)
        if not pool:
            logger.error("股票池为空,请先同步成分股(universe.sync_all_indices)")
            sys.exit(1)
        done = _done_codes(db, start)
        todo = [c for c in pool if c not in done]
        if args.limit:
            todo = todo[: args.limit]
        logger.info("池内 %d 只,已完成 %d 只,待回填 %d 只",
                    len(pool), len(done), len(todo))

        failed: list[str] = []
        t0 = time.time()
        with baostock_client.login_session():
            for i, code in enumerate(todo, 1):
                ok = False
                for attempt in range(1, RETRY + 1):
                    try:
                        ingest.backfill(db, code, start)
                        ok = True
                        break
                    except Exception:  # noqa: BLE001
                        logger.warning("回填失败 %s(第 %d/%d 次)", code, attempt, RETRY)
                        time.sleep(2 * attempt)
                if not ok:
                    failed.append(code)
                if i % 10 == 0:
                    elapsed = time.time() - t0
                    eta = elapsed / i * (len(todo) - i)
                    logger.info("进度 %d/%d,已用 %.0fs,预计剩余 %.0fs,失败 %d",
                                i, len(todo), elapsed, eta, len(failed))
                time.sleep(SLEEP_PER_CODE)

    logger.info("回填结束: 成功 %d,失败 %d %s",
                len(todo) - len(failed), len(failed), failed[:20])


if __name__ == "__main__":
    main()
