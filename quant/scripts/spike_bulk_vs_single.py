"""P0 spike: 批量日 K + 因子换算 vs 单票前复权的真网对照。

**运行前必读**
- 这是真网脚本,会访问 baostock;确认出口 IP 不在黑名单、当日配额充足。
- 全程一次登录、单进程串行;**禁止**与其他 baostock 任务叠跑
  (DATA-ARCHITECTURE.md §5 硬约束)。
- 只读不写库。

配额估算(默认参数): 最近 5 个交易日 × 2 次批量(日 K + 因子)
+ 抽样 20 只 × 2 次单票(fetch_daily_bars 前复权+不复权) ≈ **50 次**,
远低于 5 万/日上限。

对照方法(docs/baostock-bulk-ingest.md §3):
1. 逐日拉批量日 K(视为不复权)与批量因子;
2. 用 ingest.raw_to_qfq 把批量原始价合成前复权价;
3. 与单票 fetch_daily_bars(adjustflag=2/3)逐列对齐:
   open/high/low/close 比前复权,raw_close 比不复权,is_st 比 isST;
4. 相对偏差阈值用 ingest.REANCHOR_TOLERANCE,任一列超阈即 FAIL,
   说明 raw_to_qfq 的口径假设不成立,需按实测修正后再开 P2 开关。

运行: cd quant && uv run python scripts/spike_bulk_vs_single.py [--day 2026-07-24]
      [--days 5] [--sample 20] [--codes sh.600519,sz.000001,...]
"""
from __future__ import annotations

import argparse
import logging
import sys
from datetime import date, timedelta
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import pandas as pd  # noqa: E402

from app.data import baostock_client, ingest  # noqa: E402

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(name)s: %(message)s",
)
logger = logging.getLogger("spike_bulk_vs_single")

PRICE_COLS = ("open", "high", "low", "close")


def _trade_days(end: date, count: int) -> list[date]:
    """取 end 及之前的 count 个交易日(baostock 日历,1 次请求)。"""
    cal = baostock_client.fetch_trade_dates(end - timedelta(days=count * 3), end)
    days = [r.date for r in cal.itertuples() if r.is_open and r.date <= end]
    return days[-count:]


def _compare_day(day: date, sample: list[str]) -> list[dict]:
    """单日对照:批量合成前复权 vs 单票前复权,返回超阈差异记录。"""
    bulk = baostock_client.fetch_market_daily_bars(day)
    factors = baostock_client.fetch_market_adjust_factors(day)
    factor_map = {r.code: float(r.fore_factor) for r in factors.itertuples()}
    bulk = bulk[bulk["code"].isin(sample)].set_index("code")

    diffs: list[dict] = []
    for code in sample:
        if code not in bulk.index:
            diffs.append({"day": day, "code": code, "col": "*",
                          "bulk": None, "single": None, "dev": None,
                          "note": "批量结果缺该 code"})
            continue
        factor = factor_map.get(code, 1.0)
        single = baostock_client.fetch_daily_bars(code, day, day)
        if single.empty:
            diffs.append({"day": day, "code": code, "col": "*",
                          "bulk": None, "single": None, "dev": None,
                          "note": "单票结果为空(停牌?)"})
            continue
        s = single.iloc[-1]
        b = bulk.loc[code]
        for col in PRICE_COLS:
            synth = ingest.raw_to_qfq(b[col], factor)
            ref = s[col]
            if synth is None or pd.isna(ref):
                continue
            dev = abs(synth - float(ref)) / float(ref) if ref else 0.0
            if dev > ingest.REANCHOR_TOLERANCE:
                diffs.append({"day": day, "code": code, "col": col,
                              "bulk": float(b[col]), "single": float(ref),
                              "dev": round(dev, 6), "note": f"factor={factor}"})
        # raw_close(不复权)应与批量原始 close 完全同口径
        if not pd.isna(s["raw_close"]) and not pd.isna(b["close"]):
            dev = abs(float(b["close"]) - float(s["raw_close"])) / float(s["raw_close"])
            if dev > ingest.REANCHOR_TOLERANCE:
                diffs.append({"day": day, "code": code, "col": "raw_close",
                              "bulk": float(b["close"]),
                              "single": float(s["raw_close"]),
                              "dev": round(dev, 6), "note": "批量原始价口径存疑"})
        if b["is_st"] is not None and s["is_st"] is not None and bool(b["is_st"]) != bool(s["is_st"]):
            diffs.append({"day": day, "code": code, "col": "is_st",
                          "bulk": bool(b["is_st"]), "single": bool(s["is_st"]),
                          "dev": None, "note": "isST 不一致"})
    return diffs


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--day", default=None,
                        help="对照截止交易日,默认最近交易日(YYYY-MM-DD)")
    parser.add_argument("--days", type=int, default=5, help="对照交易日数")
    parser.add_argument("--sample", type=int, default=20, help="抽样只数")
    parser.add_argument("--codes", default=None,
                        help="指定对照代码(逗号分隔),指定后忽略 --sample")
    args = parser.parse_args()

    end = date.fromisoformat(args.day) if args.day else date.today()
    with baostock_client.login_session():
        days = _trade_days(end, args.days)
        logger.info("对照交易日: %s", days)
        if args.codes:
            sample = args.codes.split(",")
        else:
            first = baostock_client.fetch_market_daily_bars(days[0])
            sample = sorted(first["code"].unique())[: args.sample]
        logger.info("抽样 %d 只: %s", len(sample), sample)

        all_diffs: list[dict] = []
        for day in days:
            diffs = _compare_day(day, sample)
            all_diffs.extend(diffs)
            logger.info("%s 对照完成: %d 处超阈差异", day, len(diffs))

    if all_diffs:
        report = pd.DataFrame(all_diffs)
        print("\n===== 差异明细 =====")
        print(report.to_string(index=False))
        print(f"\nFAIL: {len(all_diffs)} 处差异超阈 "
              f"(REANCHOR_TOLERANCE={ingest.REANCHOR_TOLERANCE}),"
              "raw_to_qfq 口径假设不成立,P2 开关不得开启。")
        return 1
    print(f"\nPASS: {len(days)} 日 × {len(sample)} 只全部对齐,"
          "raw_to_qfq 公式可进入 P2 灰度。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
