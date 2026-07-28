"""只补 quant_daily_bar.is_st 一列,不重拉价格。

## 为什么需要专用脚本

`is_st` 是 alembic 0010 新增的列,既有行为 NULL。用 `backfill_pool.py
--force-rescale all` 补它是错的做法:

- 其余 8 列(open/high/low/close/raw_close/volume/amount)的值**完全不变**,
  重拉纯属浪费(实测全量约 1.7 小时);
- 每次重拉都要重走一遍重锚判定与 upsert —— 在动本该稳定的基础数据,
  为补一列新信息去触碰已验证一致的价格序列,风险不对称。

这里只 `UPDATE ... SET is_st`,不碰任何价格列、不走重锚检查。

`isST` 是 baostock 日线接口(`query_history_k_data_plus`)的字段,没有独立
接口,所以仍需按股票逐只请求 —— 但每只只发一次(前复权那次),而
`fetch_daily_bars` 会发两次(前复权 + 不复权)。

## 断点续跑

按股票为单位跳过:某 code 已有非 NULL 的 is_st 即视为已补。实测该粒度安全
——中断时不会留下「同一 code 部分行有值」的状态(单只是一次事务)。

用法:
    uv run python scripts/backfill_is_st.py                    # 全部未补的
    uv run python scripts/backfill_is_st.py --codes sh.600053  # 指定
    uv run python scripts/backfill_is_st.py --shards 4 --shard 0
"""
from __future__ import annotations

import argparse
import logging
import sys
import time
from datetime import date
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import baostock as bs  # noqa: E402
from sqlalchemy import bindparam, select, text, update  # noqa: E402

from app.data import baostock_client, ingest  # noqa: E402
from app.data.clock import today_cst  # noqa: E402
from app.db import SessionLocal  # noqa: E402
from app.models import DailyBar, Stock  # noqa: E402

logging.basicConfig(level=logging.INFO,
                    format="%(asctime)s %(levelname)s %(name)s: %(message)s")
logger = logging.getLogger("backfill_is_st")

SLEEP_PER_CODE = 0.3
RETRY = 3


def fetch_is_st(code: str, start: date, end: date) -> dict[date, bool]:
    """只取 date + isST,不取价格(前复权那次请求即可,isST 与复权方式无关)。"""
    rs = bs.query_history_k_data_plus(
        code, "date,isST", start_date=start.isoformat(),
        end_date=end.isoformat(), frequency="d", adjustflag="2",
    )
    if rs.error_code != "0":
        raise RuntimeError(f"{code}: {rs.error_code} {rs.error_msg}")
    out: dict[date, bool] = {}
    while (rs.error_code == "0") & rs.next():
        day_s, st = rs.get_row_data()
        if not day_s or st not in ("0", "1"):
            continue
        out[date.fromisoformat(day_s)] = st == "1"
    return out


def apply_is_st(db, code: str, values: dict[date, bool]) -> int:
    """按 (code, date) 更新 is_st;只动这一列。"""
    if not values:
        return 0
    rows = [{"d": day, "v": st} for day, st in values.items()]
    stmt = (
        update(DailyBar)
        .where(DailyBar.code == code, DailyBar.date == bindparam("d"))
        .values(is_st=bindparam("v"))
    )
    db.execute(stmt, rows)
    db.commit()
    return len(rows)


def pending_codes(db, shards: int, shard: int) -> list[str]:
    """未补 is_st 的股票(跳过北交所:baostock 不覆盖)。"""
    done = {
        r[0] for r in db.execute(text(
            "SELECT DISTINCT code FROM quant_daily_bar WHERE is_st IS NOT NULL"
        )).all()
    }
    codes = [
        r[0] for r in db.execute(
            select(Stock.code)
            .where(Stock.code.not_like(f"{ingest.BJ_PREFIX}%"))
            .order_by(Stock.code)
        ).all()
    ]
    todo = [c for c in codes if c not in done]
    return todo[shard::shards]


def main() -> None:
    parser = argparse.ArgumentParser(description="只补 quant_daily_bar.is_st")
    parser.add_argument("--start", default="2015-01-01")
    parser.add_argument("--codes", default="", help="逗号分隔;缺省为全部未补的")
    parser.add_argument("--shards", type=int, default=1)
    parser.add_argument("--shard", type=int, default=0)
    args = parser.parse_args()

    start = date.fromisoformat(args.start)
    end = today_cst()

    with SessionLocal() as db:
        if args.codes:
            todo = [c.strip() for c in args.codes.split(",") if c.strip()]
        else:
            todo = pending_codes(db, args.shards, args.shard)
        logger.info("待补 %d 只(分片 %d/%d)", len(todo), args.shard, args.shards)

        updated = failed = 0
        failed_codes: list[str] = []
        t0 = time.time()
        with baostock_client.login_session():
            for i, code in enumerate(todo, 1):
                for attempt in range(1, RETRY + 1):
                    try:
                        updated += apply_is_st(
                            db, code, fetch_is_st(code, start, end))
                        break
                    except Exception:  # noqa: BLE001
                        db.rollback()
                        if attempt == RETRY:
                            logger.warning("补 is_st 失败 %s", code)
                            failed += 1
                            failed_codes.append(code)
                        else:
                            time.sleep(2 * attempt)
                if i % 50 == 0:
                    elapsed = time.time() - t0
                    logger.info("进度 %d/%d,已用 %.0fs,预计剩余 %.0fs,更新 %d 行,失败 %d",
                                i, len(todo), elapsed,
                                elapsed / i * (len(todo) - i), updated, failed)
                time.sleep(SLEEP_PER_CODE)

    logger.info("结束: 更新 %d 行,失败 %d 只 %s",
                updated, failed, failed_codes[:20])


if __name__ == "__main__":
    main()
