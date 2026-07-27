"""低频初始化全市场估值、行业与财务指标。

设计目标：
- 不重拉 quant_daily_bar；估值使用按交易日的全市场千行分页，通常约 6 次请求；
- 财务指标按报告期分页，2015 年以来约 47 个报告期、通常每期约 8 次请求；
- 串行执行，分页和报告期之间默认各停 10 秒；遇 403/429 立即停止；
- 每个交易日/报告期独立事务，已完成的批次默认跳过，可安全断点续跑。

用法：
    uv run python scripts/backfill_fundamentals.py --dry-run
    uv run python scripts/backfill_fundamentals.py
    uv run python scripts/backfill_fundamentals.py --refresh
"""
from __future__ import annotations

import argparse
import logging
import sys
import time
from datetime import date
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from sqlalchemy import func, select  # noqa: E402

from app.config import settings  # noqa: E402
from app.data import fundamentals  # noqa: E402
from app.db import SessionLocal  # noqa: E402
from app.models import (  # noqa: E402
    DailyBar,
    FundamentalSnapshot,
    ValuationSnapshot,
)

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(name)s: %(message)s",
)
logger = logging.getLogger("backfill_fundamentals")


def _latest_bar_date(db) -> date:
    day = db.execute(select(func.max(DailyBar.date))).scalar_one()
    if day is None:
        raise RuntimeError("quant_daily_bar 为空，无法确定估值日期")
    return day


def _valuation_done(db, day: date) -> bool:
    return db.execute(
        select(func.count()).select_from(ValuationSnapshot).where(
            ValuationSnapshot.data_date == day,
            ValuationSnapshot.source == "eastmoney:RPT_VALUEANALYSIS_DET",
        )
    ).scalar_one() > 0


def _financial_period_done(db, period: date) -> bool:
    return db.execute(
        select(func.count()).select_from(FundamentalSnapshot).where(
            FundamentalSnapshot.report_period == period,
            FundamentalSnapshot.source
            == "eastmoney:RPT_F10_FINANCE_MAINFINADATA",
        )
    ).scalar_one() > 0


def main() -> None:
    parser = argparse.ArgumentParser(
        description="低频初始化全市场估值、行业与财务指标",
    )
    parser.add_argument("--start", default=settings.backfill_start)
    parser.add_argument("--end", default="", help="报告期截止日，默认最新日线日")
    parser.add_argument("--valuation-day", default="", help="默认最新日线日")
    parser.add_argument(
        "--request-interval", type=float,
        default=fundamentals.DEFAULT_REQUEST_INTERVAL,
        help="分页及报告期之间的最小间隔秒数，默认 10",
    )
    parser.add_argument("--skip-valuation", action="store_true")
    parser.add_argument("--skip-financials", action="store_true")
    parser.add_argument("--refresh", action="store_true", help="重拉已完成批次")
    parser.add_argument("--limit-periods", type=int, default=0, help="仅调试用")
    parser.add_argument("--dry-run", action="store_true", help="只显示待补范围")
    args = parser.parse_args()

    if args.request_interval < 5:
        parser.error("--request-interval 不得小于 5 秒")
    if args.limit_periods < 0:
        parser.error("--limit-periods 不能小于 0")

    start = date.fromisoformat(args.start)
    with SessionLocal() as db:
        latest_bar = _latest_bar_date(db)
        end = date.fromisoformat(args.end) if args.end else latest_bar
        valuation_day = (
            date.fromisoformat(args.valuation_day)
            if args.valuation_day else latest_bar
        )
        periods = fundamentals.report_periods_between(start, end)
        if not args.refresh:
            periods = [
                period for period in periods
                if not _financial_period_done(db, period)
            ]
        if args.limit_periods:
            periods = periods[:args.limit_periods]
        valuation_pending = (
            not args.skip_valuation
            and (args.refresh or not _valuation_done(db, valuation_day))
        )

        logger.info(
            "最新日线 %s；估值 %s；财务待补 %d 个报告期 [%s, %s]；请求间隔 %.1fs",
            latest_bar,
            valuation_day if valuation_pending else "已完成/跳过",
            0 if args.skip_financials else len(periods),
            start,
            end,
            args.request_interval,
        )
        if args.dry_run:
            return

        if valuation_pending:
            result = fundamentals.sync_market_valuations(
                db, valuation_day, request_interval=args.request_interval,
            )
            logger.info("估值完成: %s", result)

        if args.skip_financials:
            return
        for index, period in enumerate(periods, 1):
            if index > 1 or valuation_pending:
                time.sleep(args.request_interval)
            result = fundamentals.sync_market_financials(
                db, [period], request_interval=args.request_interval,
            )
            item = result["periods"][0]
            logger.info(
                "财务进度 %d/%d %s：写入 %d 行，请求 %d 次",
                index, len(periods), period,
                item["upserted"], item["requests"],
            )


if __name__ == "__main__":
    main()
