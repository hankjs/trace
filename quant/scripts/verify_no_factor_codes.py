"""一次性:验证存量缺因子股票(批量日 K 的 missing_factor 名单)。

对有日线但 quant_adjust_factor 无任何记录的 code,逐个用 baostock 单票
接口验证:从未除权 → 写 1.0 哨兵;有因子数据 → upsert 真实因子。
验证过的股票之后走批量日 K 正常路径,不再被跳过。

用法:
    uv run python scripts/verify_no_factor_codes.py --estimate   # 只统计
    uv run python scripts/verify_no_factor_codes.py              # 实际验证
"""
from __future__ import annotations

import argparse
import logging
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from sqlalchemy import select

from app.data import baostock_client
from app.data.ingest import verify_missing_factor_codes
from app.db import SessionLocal
from app.models import AdjustFactor, DailyBar

logging.basicConfig(level=logging.INFO, format="%(levelname)s %(message)s")
logger = logging.getLogger("verify_no_factor_codes")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--estimate", action="store_true", help="只统计不验证")
    parser.add_argument("--sleep", type=float, default=0.3, help="每股间隔秒")
    args = parser.parse_args()

    with SessionLocal() as db:
        codes = list(db.execute(
            select(DailyBar.code)
            .where(~DailyBar.code.in_(select(AdjustFactor.code).distinct()))
            .distinct().order_by(DailyBar.code)
        ).scalars())
        # 北交所 baostock 不覆盖,查了也是空,会误写哨兵——排除
        codes = [c for c in codes if not c.startswith("bj.")]
        logger.info("有日线但无因子记录(沪深): %d 只", len(codes))
        if args.estimate:
            print("\n".join(codes[:50]))
            if len(codes) > 50:
                print(f"... 共 {len(codes)} 只")
            return
        if not codes:
            return

    with SessionLocal() as db, baostock_client.login_session():
        result = verify_missing_factor_codes(
            db, codes, sleep_per_code=args.sleep, max_codes=len(codes))
        db.commit()
    logger.info(
        "完成: 从未除权(哨兵) %d, 补真实因子 %d, 剩余 %d",
        len(result["verified_none"]), len(result["synced"]),
        len(result["remaining"]))


if __name__ == "__main__":
    main()
