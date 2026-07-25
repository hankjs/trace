"""抽查回填后的前复权尺度一致性(REVIEW §3.1 的验收手段)。

判据说明:不能用「复权因子跨日跳变」当异常——送转股会让前复权因子按比例
跳变(如 10 送 20 时因子 0.29 -> 0.88,正好 3 倍),那是正确行为。

真正的尺度混接特征是:相邻交易日 `close` 巨变,而 `raw_close`(不复权价)
基本不变。raw_close 只在除权日跟着实际价格走,若它平稳而 close 突变,
说明这两行数据来自不同的复权尺度。
"""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from sqlalchemy import text  # noqa: E402

from app.db import SessionLocal  # noqa: E402

SAMPLE = 20
CLOSE_JUMP = 0.30    # close 相对跳变阈值
RAW_STABLE = 0.11    # raw_close 视为「基本不变」的上限(留出涨跌停 10%)


def main() -> None:
    with SessionLocal() as db:
        codes = [
            r[0] for r in db.execute(text(
                "SELECT code FROM (SELECT code, MIN(date) f, COUNT(*) n "
                "FROM quant_daily_bar GROUP BY code) t "
                "WHERE f <= '2015-01-11' AND n > 2000 "
                "ORDER BY RAND() LIMIT :lim"), {"lim": SAMPLE}).all()
        ]
        suspects: list[tuple[str, int, str]] = []
        for code in codes:
            rows = db.execute(text(
                "SELECT date, close, raw_close FROM quant_daily_bar "
                "WHERE code = :c AND raw_close > 0 AND close > 0 "
                "ORDER BY date"), {"c": code}).all()
            hits, first = 0, ""
            for i in range(1, len(rows)):
                c0, c1 = float(rows[i - 1][1]), float(rows[i][1])
                r0, r1 = float(rows[i - 1][2]), float(rows[i][2])
                if abs(c1 - c0) / c0 > CLOSE_JUMP and abs(r1 - r0) / r0 < RAW_STABLE:
                    hits += 1
                    first = first or str(rows[i][0])
            if hits:
                suspects.append((code, hits, first))

        print(f"抽查 {len(codes)} 只(起点<=2015 且 >2000 行)")
        print(f"判据: close 跳变 >{CLOSE_JUMP:.0%} 而 raw_close 变动 <{RAW_STABLE:.0%}")
        if suspects:
            print("\n❌ 疑似尺度混接:")
            for code, hits, first in suspects:
                print(f"  {code}: {hits} 处,首次 {first}")
            sys.exit(1)
        print("\n✅ 未发现跨尺度跳空")


if __name__ == "__main__":
    main()
