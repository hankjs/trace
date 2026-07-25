"""股票池历史回填:池内全部股票 3 年前复权日线。

特性:
- 断点续跑:最早日线已覆盖请求起点且最近日期在 10 天内的 code 直接跳过;
- **走重锚检查**:库中已有历史时先校验复权尺度,尺度变了强制全量重拉,
  不再像旧版直调 ingest.backfill 那样把新尺度 bar 静默接到旧尺度历史上;
- `--force-rescale`:忽略断点续跑,强制重拉(可指定代码,或 all 全部);
- 失败重试:单只最多 3 次,间隔递增;单只失败不影响整体;
- baostock 限速友好:全程复用一次登录,每只间隔 sleep。

运行: cd quant && uv run python scripts/backfill_pool.py [--limit N] [--years 3]
全市场回填: uv run python scripts/backfill_pool.py --all --start 2019-01-01
修复尺度错乱: uv run python scripts/backfill_pool.py --force-rescale sh.600519,sz.000001
             uv run python scripts/backfill_pool.py --all --force-rescale all
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
from app.data.clock import today_cst  # noqa: E402
from app.db import Base, SessionLocal, engine  # noqa: E402
from app import models  # noqa: E402,F401 - 确保建表元数据注册
from app.models import DailyBar, Stock  # noqa: E402

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(name)s: %(message)s",
)
logger = logging.getLogger("backfill_pool")

RECENT_DAYS = 10        # 最近数据在此天数内视为已完成
RETRY = 3
SLEEP_PER_CODE = 0.3    # 限速友好


def _done_codes(db, start: date) -> set[str]:
    """已回填完成的 code:3 年窗口内条数 >= MIN_BARS 且最近日期足够新"""
    cutoff = today_cst() - timedelta(days=RECENT_DAYS)
    rows = db.execute(
        select(DailyBar.code, func.min(DailyBar.date), func.max(DailyBar.date))
        .where(DailyBar.date >= start)
        .group_by(DailyBar.code)
    ).all()
    start_tolerance = start + timedelta(days=RECENT_DAYS)
    return {
        code for code, first_day, last_day in rows
        if first_day <= start_tolerance and last_day >= cutoff
    }


def backfill_checked(db, code: str, start: date, end: date | None = None,
                     force: bool = False) -> int:
    """带重锚校验的回填(委托 ingest.safe_backfill)。

    旧版这里直调 `ingest.backfill`,完全绕过重锚检查,配合 `_done_codes`
    的永久 done 标记会让尺度错乱的股票再也修不回来(REVIEW §3.1 第 2 点)。
    """
    return ingest.safe_backfill(db, code, start, end, force=force)


def _parse_force_rescale(raw: str) -> tuple[bool, set[str]]:
    """--force-rescale 解析:'all' → 全部强制;逗号分隔代码 → 指定强制。"""
    if not raw:
        return False, set()
    if raw.strip().lower() == "all":
        return True, set()
    codes = {c.strip().lower() for c in raw.split(",") if c.strip()}
    return False, codes


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--limit", type=int, default=0, help="只回填前 N 只(调试用)")
    parser.add_argument("--years", type=int, default=3)
    parser.add_argument("--start", default="",
                        help="回填起点 YYYY-MM-DD(优先于 --years)")
    parser.add_argument("--all", action="store_true",
                        help="回填 quant_stock 全市场名录,而非当前成分池")
    parser.add_argument("--force-rescale", default="",
                        help="强制重拉:'all' 或逗号分隔代码。忽略断点续跑,"
                             "用于修复复权尺度错乱的股票")
    parser.add_argument("--shards", type=int, default=1,
                        help="并发分片总数(多进程各自独立 baostock 会话)")
    parser.add_argument("--shard", type=int, default=0,
                        help="本进程分片序号 0..N-1(交错切分,负载均衡)")
    args = parser.parse_args()

    if args.years <= 0:
        parser.error("--years 必须大于 0")
    if args.shards <= 0:
        parser.error("--shards 必须大于 0")
    if not 0 <= args.shard < args.shards:
        parser.error("--shard 必须在 0 到 --shards-1 之间")
    force_all, force_codes = _parse_force_rescale(args.force_rescale)

    Base.metadata.create_all(engine)
    start = (date.fromisoformat(args.start) if args.start
             else today_cst() - timedelta(days=args.years * 365))
    if start > today_cst():
        parser.error("回填起点不能晚于今天")

    with SessionLocal() as db:
        if args.all:
            pool = sorted(r[0] for r in db.execute(select(Stock.code)).all())
        else:
            pool = universe.current_pool(db)
        if force_codes:
            # 显式指定的代码即便不在池内也要能修
            pool = sorted(set(pool) | force_codes)
        if not pool:
            logger.error("股票池为空,请先同步成分股(universe.sync_all_indices)")
            sys.exit(1)
        if force_all:
            done: set[str] = set()
        else:
            done = _done_codes(db, start) - force_codes
        todo = [c for c in pool if c not in done]
        todo = todo[args.shard::args.shards]  # 交错分片,各进程互不重叠
        if args.limit:
            todo = todo[: args.limit]
        logger.info("池内 %d 只,已完成 %d 只,分片 %d/%d 待回填 %d 只%s",
                    len(pool), len(done), args.shard, args.shards, len(todo),
                    "(全部强制重拉)" if force_all else
                    f"(强制重拉 {len(force_codes)} 只)" if force_codes else "")

        failed: list[str] = []
        t0 = time.time()
        with baostock_client.login_session():
            for i, code in enumerate(todo, 1):
                ok = False
                force = force_all or code in force_codes
                for attempt in range(1, RETRY + 1):
                    try:
                        backfill_checked(db, code, start, force=force)
                        ok = True
                        break
                    except Exception:  # noqa: BLE001
                        db.rollback()  # 失效事务不得污染后续股票
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
