"""数据入库:历史回填、盘后增量、快照落库、双源对账。"""
from __future__ import annotations

import logging
import time
from datetime import date, datetime, timedelta

import pandas as pd
from sqlalchemy import delete, func, select
from sqlalchemy.dialects.mysql import insert as mysql_insert
from sqlalchemy.dialects.sqlite import insert as sqlite_insert
from sqlalchemy.orm import Session

from ..models import (AdjustFactor, DailyBar, FundamentalSnapshot, Snapshot,
                      Stock, ValuationSnapshot, WatchlistItem)
from . import akshare_client, baostock_client
from .clock import naive_now_cst, today_cst

logger = logging.getLogger(__name__)

# 名称含以下子串即视为风险警示股(*ST/ST/退市整理期)
ST_NAME_MARKERS = ("ST", "*ST", "退")


def is_st_name(name: str | None) -> bool:
    """按名称判断风险警示。名称是 ST 的第一手信号,但不是唯一信号。"""
    if not name:
        return False
    upper = name.upper().replace(" ", "")
    return any(marker in upper for marker in ST_NAME_MARKERS)


def upsert_stock(db: Session, code: str, name: str = "", industry: str = "",
                 is_watch: bool | None = None) -> Stock:
    stock = db.get(Stock, code)
    if stock is None:
        stock = Stock(code=code, name=name, industry=industry,
                      is_watch=bool(is_watch))
        db.add(stock)
    else:
        if name:
            stock.name = name
        if industry:
            stock.industry = industry
        if is_watch is not None:
            stock.is_watch = is_watch
    if name:
        # 改名为 *ST 的股票必须同步刷新标记,否则 ST 过滤永远漏它
        stock.is_st = is_st_name(name)
    db.commit()
    return stock


def import_stock_list(db: Session) -> dict:
    """导入/更新全市场股票名录。

    akshare 出代码与名称(含 ST 改名),baostock query_stock_basic 出上市/退市
    日期与上市状态。两源合并后 upsert:
    - 已存在的记录也要更新 name / is_st(原实现只 insert,改名为 *ST 的股票
      永远不会被 ST 过滤命中,退市股也不标记 —— REVIEW §3.5);
    - 退市只标 delist_date,不删行(历史回测需要);
    - is_st 同时看名称与交易所上市状态,不只靠子串。
    """
    df = akshare_client.fetch_stock_list()
    basic = _fetch_stock_basic_map()

    existing = {s.code: s for s in db.execute(select(Stock)).scalars().all()}
    inserted = updated = 0
    seen: set[str] = set()
    for row in df.itertuples():
        code = row.code
        seen.add(code)
        meta = basic.get(code, {})
        name = row.name or meta.get("name") or ""
        stock = existing.get(code)
        if stock is None:
            stock = Stock(code=code, name=name)
            db.add(stock)
            existing[code] = stock
            inserted += 1
        else:
            updated += 1
            if name:
                stock.name = name
        _apply_lifecycle(stock, name, meta)

    # akshare 列表只含在市股票:库中有、名录里没有的按退市处理(不删行)
    delisted = 0
    for code, stock in existing.items():
        if code in seen:
            continue
        meta = basic.get(code, {})
        if stock.delist_date is None:
            stock.delist_date = meta.get("delist_date") or today_cst()
            delisted += 1
        # 已退市即不可交易,统一纳入 is_st 过滤口径
        stock.is_st = True
    db.commit()
    logger.info("股票名录导入: 新增 %d,更新 %d,新标退市 %d",
                inserted, updated, delisted)
    return {"imported": inserted, "updated": updated, "delisted": delisted,
            "total": len(seen)}


def _fetch_stock_basic_map() -> dict[str, dict]:
    """baostock 证券资料 -> {code: {...}};失败不阻断名录导入。"""
    try:
        basic = baostock_client.fetch_stock_basic()
    except Exception:  # noqa: BLE001 - 数据源异常降级为仅 akshare 名录
        logger.warning("baostock 证券资料获取失败,本次仅按 akshare 名录更新",
                       exc_info=True)
        return {}
    out: dict[str, dict] = {}
    for row in basic.itertuples():
        if str(getattr(row, "type", "")) not in ("", "1"):
            continue  # 只要股票,过滤指数/可转债/ETF
        out[row.code] = {
            "name": row.name,
            "list_date": None if pd.isna(row.list_date) else row.list_date,
            "delist_date": None if pd.isna(row.delist_date) else row.delist_date,
            "status": str(getattr(row, "status", "")),
        }
    return out


def _apply_lifecycle(stock: Stock, name: str, meta: dict) -> None:
    """写入 list_date / delist_date / is_st。"""
    if meta.get("list_date") and stock.list_date != meta["list_date"]:
        stock.list_date = meta["list_date"]
    if meta.get("delist_date"):
        stock.delist_date = meta["delist_date"]
    elif meta.get("status") == "0" and stock.delist_date is None:
        # baostock 标记为已退市但没给日期:标当天,保留历史行
        stock.delist_date = today_cst()
    # ST 判定:名称子串 + 交易所上市状态(status=0 视为已终止上市)
    stock.is_st = bool(is_st_name(name) or stock.delist_date is not None)


def backfill_list_dates(db: Session) -> dict:
    """回填历史 list_date(全A 口径 point-in-time 解析的前置)。

    agent-pool 的 kind='all' 池按 list_date 解析成员,缺失会静默漏票。
    baostock 拿不到日期的,退化为该股在 quant_daily_bar 里的最早一根日线。
    """
    basic = _fetch_stock_basic_map()
    stocks = db.execute(select(Stock)).scalars().all()
    from_basic = from_bars = still_missing = 0
    missing_codes: list[str] = []
    for stock in stocks:
        if stock.list_date is not None:
            continue
        meta = basic.get(stock.code, {})
        if meta.get("list_date"):
            stock.list_date = meta["list_date"]
            from_basic += 1
            continue
        missing_codes.append(stock.code)
    if missing_codes:
        rows = db.execute(
            select(DailyBar.code, func.min(DailyBar.date))
            .where(DailyBar.code.in_(missing_codes))
            .group_by(DailyBar.code)
        ).all()
        first_bar = {code: day for code, day in rows}
        for code in missing_codes:
            stock = db.get(Stock, code)
            if stock is None:
                continue
            if code in first_bar:
                stock.list_date = first_bar[code]
                from_bars += 1
            else:
                still_missing += 1
    db.commit()
    logger.info("list_date 回填: baostock %d,日线兜底 %d,仍缺失 %d",
                from_basic, from_bars, still_missing)
    return {"from_basic": from_basic, "from_bars": from_bars,
            "missing": still_missing}


def upsert_valuations_from_daily_bulk(db: Session, day: date,
                                      bars: pd.DataFrame) -> int:
    """把批量日 K 里的 peTTM/pb/psTTM 落入 quant_valuation_snapshot。

    source=baostock_k_daily,available_date=data_date=day(日频点时,无公告滞后)。
    与东财全市场估值并存:同 (code,data_date,available_date) 覆盖更新。
    """
    if bars.empty:
        return 0
    need = [c for c in ("pe_ttm", "pb", "ps_ttm") if c in bars.columns]
    if not need:
        return 0
    usable = bars.dropna(subset=need, how="all")
    if usable.empty:
        return 0
    rows = []
    for r in usable.itertuples():
        pe = getattr(r, "pe_ttm", None)
        pb = getattr(r, "pb", None)
        ps = getattr(r, "ps_ttm", None)
        if pe is not None and pd.isna(pe):
            pe = None
        if pb is not None and pd.isna(pb):
            pb = None
        if ps is not None and pd.isna(ps):
            ps = None
        if pe is None and pb is None and ps is None:
            continue
        rows.append({
            "code": r.code,
            "data_date": day,
            "available_date": day,
            "report_period": None,
            "source": "baostock_k_daily",
            "pe_ttm": None if pe is None else float(pe),
            "pb": None if pb is None else float(pb),
            "ps_ttm": None if ps is None else float(ps),
            "dividend_yield": None,
            "total_market_cap": None,
        })
    if not rows:
        return 0
    # 按批 upsert,避免单次包过大
    updated_cols = ("pe_ttm", "pb", "ps_ttm", "source", "report_period",
                    "dividend_yield", "total_market_cap")
    dialect = db.get_bind().dialect.name
    n = 0
    for i in range(0, len(rows), 500):
        chunk = rows[i:i + 500]
        if dialect == "sqlite":
            stmt = sqlite_insert(ValuationSnapshot).values(chunk)
            stmt = stmt.on_conflict_do_update(
                index_elements=[
                    ValuationSnapshot.code, ValuationSnapshot.data_date,
                    ValuationSnapshot.available_date,
                ],
                set_={c: getattr(stmt.excluded, c) for c in updated_cols},
            )
        else:
            stmt = mysql_insert(ValuationSnapshot).values(chunk)
            stmt = stmt.on_duplicate_key_update(
                **{c: getattr(stmt.inserted, c) for c in updated_cols}
            )
        db.execute(stmt)
        n += len(chunk)
    db.commit()
    return n


def upsert_bars(db: Session, code: str, df: pd.DataFrame) -> int:
    """日线 upsert(code+date 唯一键)。df 可无 code 列(则用参数 code)。"""
    if df.empty:
        return 0
    work = df.copy()
    if "code" not in work.columns:
        work["code"] = code
    return upsert_bars_frame(db, work)


def upsert_bars_frame(db: Session, df: pd.DataFrame) -> int:
    """多 code 日线批量 upsert(code+date 唯一键)。

    重锚修复依赖「同 code+date 的旧行被新尺度覆盖」;必须真正 upsert。
    按批提交,避免单次包过大;比按只循环快一个数量级。
    """
    if df.empty:
        return 0
    need = {"code", "date", "open", "high", "low", "close"}
    if not need.issubset(df.columns):
        raise ValueError(f"upsert_bars_frame 缺列: {need - set(df.columns)}")
    work = df.dropna(subset=["close"])
    if work.empty:
        return 0

    rows = []
    for r in work.itertuples():
        raw = getattr(r, "raw_close", None)
        is_st = getattr(r, "is_st", None)
        rows.append({
            "code": r.code,
            "date": r.date,
            "open": float(r.open),
            "high": float(r.high),
            "low": float(r.low),
            "close": float(r.close),
            "raw_close": None if raw is None or pd.isna(raw) else float(raw),
            "volume": 0.0 if pd.isna(getattr(r, "volume", 0)) else float(r.volume),
            "amount": 0.0 if pd.isna(getattr(r, "amount", 0)) else float(r.amount),
            "is_st": (None if is_st is None or (isinstance(is_st, float) and pd.isna(is_st))
                      else bool(is_st)),
        })
    updated_cols = ("open", "high", "low", "close", "raw_close", "volume",
                    "amount", "is_st")
    dialect = db.get_bind().dialect.name
    total = 0
    for i in range(0, len(rows), 800):
        chunk = rows[i:i + 800]
        if dialect == "sqlite":
            stmt = sqlite_insert(DailyBar).values(chunk)
            stmt = stmt.on_conflict_do_update(
                index_elements=[DailyBar.code, DailyBar.date],
                set_={c: getattr(stmt.excluded, c) for c in updated_cols},
            )
        else:
            stmt = mysql_insert(DailyBar).values(chunk)
            stmt = stmt.on_duplicate_key_update(
                **{c: getattr(stmt.inserted, c) for c in updated_cols}
            )
        db.execute(stmt)
        total += len(chunk)
    db.commit()
    return total


def upsert_adjust_factors(db: Session, code: str, df: pd.DataFrame,
                          source: str = "baostock") -> int:
    """复权因子 upsert(code + divid_operate_date 主键)。

    因子是权威事实,理论上只增不改;但 baostock 偶有修订,故用 upsert
    而非 insert-ignore,让修订能覆盖旧值。

    source 区分可信度:'baostock' 是 query_adjust_factor 的权威 6 位小数值;
    'sina' 是北交所从 close/raw_close 自算的,精度受 DECIMAL(12,4) 限制
    (约 4~5 位有效)。审计时两者不可混为一谈,见 alembic 0008。
    """
    if df.empty:
        return 0
    rows = [
        {
            "code": code,
            "divid_operate_date": r.divid_operate_date,
            "fore_factor": float(r.fore_factor),
            "back_factor": (None if pd.isna(r.back_factor)
                            else float(r.back_factor)),
            "source": source,
        }
        for r in df.itertuples()
    ]
    updated_cols = ("fore_factor", "back_factor", "source")
    if db.get_bind().dialect.name == "sqlite":
        stmt = sqlite_insert(AdjustFactor).values(rows)
        stmt = stmt.on_conflict_do_update(
            index_elements=[AdjustFactor.code, AdjustFactor.divid_operate_date],
            set_={c: getattr(stmt.excluded, c) for c in updated_cols},
        )
    else:
        stmt = mysql_insert(AdjustFactor).values(rows)
        stmt = stmt.on_duplicate_key_update(
            **{c: getattr(stmt.inserted, c) for c in updated_cols}
        )
    db.execute(stmt)
    db.commit()
    return len(rows)


def sync_adjust_factors(db: Session, codes: list[str] | None = None,
                        start: date | str = "2015-01-01",
                        end: date | str | None = None,
                        sleep_per_code: float = 0.0) -> dict:
    """全市场复权因子采集。

    因子按除权日稀疏返回(实测 sh.600519 的 2808 行日线只对应 17 个除权日),
    所以整轮采集比日线回填轻得多。

    空响应视为「该股无分红送转」,不清空已有数据 —— 数据源抖动不该被当成
    「因子被撤销」(同 sync_index_members 对空响应的处理)。
    """
    end = end or today_cst()
    if codes is None:
        # 跳过北交所:baostock 完全不覆盖(bj. 前缀直接报 10004011),
        # 不跳会白发 330 次注定失败的请求。它们的因子由 sync_bj_market
        # 从 close/raw_close 自算并标 source='sina'(见 alembic 0008)。
        codes = [
            r[0] for r in db.execute(
                select(Stock.code).where(Stock.code.not_like(f"{BJ_PREFIX}%"))
            ).all()
        ]
    total = upserted = empty = failed = 0
    failed_codes: list[str] = []
    with baostock_client.login_session():
        for code in codes:
            total += 1
            try:
                df = baostock_client.fetch_adjust_factors(code, start, end)
            except Exception:  # noqa: BLE001
                logger.warning("复权因子采集失败 %s", code, exc_info=True)
                failed += 1
                failed_codes.append(code)
                db.rollback()
                continue
            if df.empty:
                empty += 1
            else:
                upserted += upsert_adjust_factors(db, code, df)
            if sleep_per_code:
                time.sleep(sleep_per_code)
    logger.info("复权因子采集: %d 只,写入 %d 行,无分红 %d 只,失败 %d 只",
                total, upserted, empty, failed)
    return {"total": total, "upserted": upserted, "empty": empty,
            "failed": failed, "failed_codes": failed_codes[:20]}


def backfill(db: Session, code: str, start: date | str, end: date | str | None = None) -> int:
    """历史回填:baostock 拉 [start, end] 全量日线并 upsert"""
    end = end or today_cst()
    df = baostock_client.fetch_daily_bars(code, start, end)
    n = upsert_bars(db, code, df)
    logger.info("回填 %s [%s, %s]: %d 条", code, start, end, n)
    return n


# 复权因子(close/raw_close)相对偏差阈值。价格列改 DECIMAL 后可收紧,
# 见 logs/decisions-data.md「重锚阈值」。
REANCHOR_TOLERANCE = 0.001
# 无 raw_close 可用时退化为 close 直接比对的阈值(同尺度下应完全相等)
CLOSE_FALLBACK_TOLERANCE = 0.001
# 自算因子(source='sina',北交所)的审计容差。必须比权威值宽:
# 它从 close/raw_close 反推,精度受 DECIMAL(12,4) 限制(低价股噪声 P99
# 约 0.19%),且低于 FACTOR_CHANGE_TOLERANCE 的小额分红不会被记录。
# 代价明确:自算源只保证检出 >1% 的尺度错乱,小额分红漏记不报警。
DERIVED_FACTOR_TOLERANCE = 0.012


class ReanchorVerdict:
    """重锚判定结果"""

    __slots__ = ("reanchored", "reason", "detail")

    def __init__(self, reanchored: bool, reason: str, detail: str = "") -> None:
        self.reanchored = reanchored
        self.reason = reason
        self.detail = detail


def _factor(close: float | None, raw_close: float | None) -> float | None:
    """当日复权系数 = 前复权价 / 不复权价。

    raw_close 不随分红送转变化,同一天的系数刻画的是「这批数据的复权尺度」,
    比直接比对 close 稳健:分红后 baostock 会回溯调整全部历史前复权价,
    close 变了但 raw_close 不变,系数随之变化,一比即中。
    """
    if close is None or raw_close is None:
        return None
    try:
        c, r = float(close), float(raw_close)
    except (TypeError, ValueError):
        return None
    if not (c > 0 and r > 0):
        return None
    return c / r


BJ_PREFIX = "bj."          # 北交所:baostock 不覆盖,走 akshare 新浪源
# 因子变化判定阈值。必须远大于 DECIMAL(12,4) 的舍入噪声,而噪声大小取决于
# **股价量级**:close/raw_close 各只有 4 位小数,低价股的相对精度差得多。
# 实测 bj.920000(股价约 10 元)相邻因子的相对变化:
#   P50 = 2.1e-04, P90 = 1.1e-03, P99 = 1.9e-03  ← 全是噪声
#   真实除权 4 次,跳变量级 1.4e-02 ~ 2.2e-01     ← 相差两个数量级
# 早先用茅台(1300 元)验证时噪声只有 1e-06,故 1e-4 看似够用 —— 那是被高价股
# 掩盖的错误结论,对低价股会把噪声全当成除权(实测产出 898 个假除权日)。
FACTOR_CHANGE_TOLERANCE = 0.01


def is_bj_code(code: str) -> bool:
    """北交所代码。baostock 完全不覆盖(见 alembic 0008)。"""
    return code.startswith(BJ_PREFIX)


def derive_adjust_factors(df: pd.DataFrame) -> pd.DataFrame:
    """从 close/raw_close 反推因子,只保留变化点(除权日)。

    因子在除权日之间是常数,故按变化点稀疏化 —— 与 baostock 按除权日返回的
    形态一致。阈值 1e-4 而非 0:close 与 raw_close 都是 DECIMAL(12,4),两个
    4 位小数相除会在小数第 6 位抖动,那是舍入噪声不是除权
    (实测真实除权的因子跳变远大于此,1e-4 与 1e-3 结果几乎相同)。
    """
    cols = ["divid_operate_date", "fore_factor"]
    if df.empty or "raw_close" not in df.columns:
        return pd.DataFrame(columns=cols)
    usable = df.dropna(subset=["close", "raw_close"]).sort_values("date")
    usable = usable[(usable["close"] > 0) & (usable["raw_close"] > 0)]
    if usable.empty:
        return pd.DataFrame(columns=cols)

    rows: list[dict] = []
    prev: float | None = None
    for r in usable.itertuples():
        factor = float(r.close) / float(r.raw_close)
        if prev is None or abs(factor - prev) / prev > FACTOR_CHANGE_TOLERANCE:
            rows.append({"divid_operate_date": r.date, "fore_factor": factor})
            prev = factor

    # 末行锚点:额外存最新交易日的实际因子。
    #
    # 为什么需要它:单一相对阈值无法干净分离低价股的舍入噪声与小额分红。
    # 实测 bj.920000 在 2026-05-25 有次真实除权(0.99474 -> 1.0),相对变化
    # 仅 0.53%,低于 1% 阈值被当噪声丢掉 —— 于是因子表最后一行停在一年前,
    # 审计核对最新 bar 时取到过时因子,误报「尺度错乱」(实测 87/330 只)。
    # DECIMAL(12,4) 对低价股的噪声 P99 约 0.19%,与小额分红信号量级重叠,
    # 调阈值只是在「漏报噪声」和「漏报除权」之间换一头错。
    #
    # 锚点让审计最常做的操作(核对最新 bar)变精确;中间日期的小额除权仍
    # 可能漏记,故自算因子只保证检出 >1% 的尺度错乱,见 audit 的分源容差。
    last = usable.iloc[-1]
    last_factor = float(last.close) / float(last.raw_close)
    if not rows or rows[-1]["divid_operate_date"] != last.date:
        rows.append({"divid_operate_date": last.date,
                     "fore_factor": last_factor})
    return pd.DataFrame(rows, columns=cols)


def backfill_bj(db: Session, code: str, start: date,
                end: date | None = None) -> dict:
    """北交所单只回填:日线 + 自算因子。

    baostock 不覆盖北交所,故不能走 safe_backfill 的重锚检查路径(它依赖
    baostock 权威因子)。这里自算因子并标记 source='sina',让审计能区分
    可信度 —— 自算值精度受 DECIMAL(12,4) 限制,低于权威值。
    """
    end = end or today_cst()
    df = akshare_client.fetch_bj_daily_bars(code, start, end)
    if df.empty:
        return {"code": code, "bars": 0, "factors": 0}
    bars = upsert_bars(db, code, df)
    factors = derive_adjust_factors(df)
    n_factors = 0
    if not factors.empty:
        factors = factors.assign(code=code, back_factor=None)
        n_factors = upsert_adjust_factors(db, code, factors, source="sina")
    return {"code": code, "bars": bars, "factors": n_factors}


def sync_bj_market(db: Session, start: date | str = "2015-01-01",
                   end: date | None = None,
                   sleep_per_code: float = 1.0) -> dict:
    """全部北交所标的的日线与因子采集。

    新浪源比 baostock 更容易限流(实测东财接口整段不可用时新浪仍可用,
    但也需退避),故默认每只间隔 1 秒。
    """
    if isinstance(start, str):
        start = date.fromisoformat(start)
    codes = [
        r[0] for r in db.execute(
            select(Stock.code).where(Stock.code.like(f"{BJ_PREFIX}%"))
            .order_by(Stock.code)
        ).all()
    ]
    total = bars = factors = empty = failed = 0
    failed_codes: list[str] = []
    for code in codes:
        total += 1
        try:
            r = backfill_bj(db, code, start, end)
        except Exception:  # noqa: BLE001
            logger.warning("北交所回填失败 %s", code, exc_info=True)
            failed += 1
            failed_codes.append(code)
            db.rollback()
            continue
        if r["bars"] == 0:
            empty += 1
        bars += r["bars"]
        factors += r["factors"]
        if sleep_per_code:
            time.sleep(sleep_per_code)
    logger.info("北交所采集: %d 只,日线 %d 行,因子 %d 行,无数据 %d 只,失败 %d 只",
                total, bars, factors, empty, failed)
    return {"total": total, "bars": bars, "factors": factors,
            "empty": empty, "failed": failed, "failed_codes": failed_codes[:20]}


def audit_scale_against_factors(db: Session, code: str) -> ReanchorVerdict:
    """用 quant_adjust_factor 的**权威**因子核对库中历史的复权尺度。

    为什么需要它:`detect_reanchor` 是「库中反推因子 vs 新拉反推因子」的
    自比对,两边都来自 close/raw_close。若某股入库时历史就已错乱,两边会
    一致地错下去 —— 拿反推值当基准是循环论证,检测不出既存的错乱。

    这里换成独立基准:库中 close/raw_close 反推的因子,应当等于权威因子表
    在该日生效的值(前复权基准为最新日,故取 divid_operate_date <= 该日的
    最后一个 fore_factor)。不等即说明库中价格的尺度与权威因子对不上。

    返回 `no_factors` 表示因子表还没这只股票的数据,调用方应降级到
    `detect_reanchor`(不能因为缺基准就假定尺度正确)。
    """
    factors = db.execute(
        select(AdjustFactor.divid_operate_date, AdjustFactor.fore_factor,
               AdjustFactor.source)
        .where(AdjustFactor.code == code)
        .order_by(AdjustFactor.divid_operate_date)
    ).all()
    if not factors:
        return ReanchorVerdict(False, "no_factors")

    # 容差按来源区分:baostock 是权威 6 位小数值,可用严格阈值;'sina' 是
    # 从 close/raw_close 自算的,精度受 DECIMAL(12,4) 限制,且小额分红可能
    # 未被记录(见 derive_adjust_factors 的末行锚点说明)。对它用严格阈值
    # 会大量误报,故放宽 —— 代价是自算源只保证检出 >1% 的尺度错乱。
    is_derived = any(src == "sina" for _, _, src in factors)
    tolerance = DERIVED_FACTOR_TOLERANCE if is_derived else REANCHOR_TOLERANCE

    # 取库中最新一根有 raw_close 的 bar 核对:前复权基准是最新日,
    # 该日的因子应当等于最后一个除权日的 fore_factor。
    row = db.execute(
        select(DailyBar.date, DailyBar.close, DailyBar.raw_close)
        .where(DailyBar.code == code, DailyBar.raw_close.is_not(None))
        .order_by(DailyBar.date.desc()).limit(1)
    ).first()
    if row is None:
        return ReanchorVerdict(False, "no_history")

    stored_factor = _factor(row.close, row.raw_close)
    if stored_factor is None:
        return ReanchorVerdict(False, "unverifiable")

    # 该 bar 日期生效的因子
    effective = None
    for divid_date, fore, _src in factors:
        if divid_date <= row.date:
            effective = float(fore)
        else:
            break
    if effective is None or effective <= 0:
        # 该 bar 早于首个除权日,因子未覆盖,无从核对
        return ReanchorVerdict(False, "no_factors")

    dev = abs(stored_factor - effective) / effective
    src_label = "自算因子" if is_derived else "权威因子"
    detail = (f"{row.date} 库中系数 {stored_factor:.6f} vs "
              f"{src_label} {effective:.6f}")
    if dev > tolerance:
        return ReanchorVerdict(True, "authoritative_mismatch", detail)
    return ReanchorVerdict(False, "authoritative_match", detail)


def detect_reanchor(db: Session, code: str, df: pd.DataFrame) -> ReanchorVerdict:
    """判定本批数据与库中历史是否处于同一前复权尺度。

    三种结论:
    - `no_history`:该 code 库中无任何日线,不存在混接风险,直接 upsert;
    - `no_overlap`:库中有历史但与本批无任何重叠日 —— **无法校验尺度**,
      判定为需要全量回填(原实现 `if stored and ...` 在这里静默放过,
      把新尺度 bar 直接接到旧尺度历史上,REVIEW §3.1);
    - `factor_mismatch` / `close_mismatch`:重叠日的复权系数(或退化的 close)
      偏差超阈值,判定为发生了重锚。
    """
    if df.empty:
        return ReanchorVerdict(False, "empty")

    first_date, last_date = df["date"].min(), df["date"].max()
    history_min = db.execute(
        select(func.min(DailyBar.date)).where(DailyBar.code == code)
    ).scalar()
    if history_min is None:
        return ReanchorVerdict(False, "no_history")

    stored_rows = db.execute(
        select(DailyBar.date, DailyBar.close, DailyBar.raw_close).where(
            DailyBar.code == code,
            DailyBar.date >= first_date,
            DailyBar.date <= last_date,
        )
    ).all()
    if not stored_rows:
        return ReanchorVerdict(
            True, "no_overlap",
            f"库中历史自 {history_min} 起,但与本批 [{first_date}, {last_date}] "
            f"无重叠日,无法校验复权尺度",
        )

    incoming = {
        r.date: (r.close, r.raw_close)
        for r in df.itertuples()
        if not pd.isna(r.close)
    }
    worst_factor: tuple[float, str] | None = None
    worst_close: tuple[float, str] | None = None
    for day, stored_close, stored_raw in stored_rows:
        new = incoming.get(day)
        if new is None:
            continue
        new_close, new_raw = new
        f_old = _factor(stored_close, stored_raw)
        f_new = _factor(new_close, None if pd.isna(new_raw) else new_raw)
        if f_old is not None and f_new is not None:
            dev = abs(f_new - f_old) / f_old
            if worst_factor is None or dev > worst_factor[0]:
                worst_factor = (
                    dev,
                    f"{day} 复权系数 库中 {f_old:.6f} vs 新拉 {f_new:.6f}",
                )
            continue
        # raw_close 缺失(老数据/停牌行)时退化为 close 比对
        if stored_close and float(stored_close) > 0 and not pd.isna(new_close):
            dev = abs(float(new_close) - float(stored_close)) / float(stored_close)
            if worst_close is None or dev > worst_close[0]:
                worst_close = (
                    dev,
                    f"{day} close 库中 {float(stored_close):.4f} "
                    f"vs 新拉 {float(new_close):.4f}",
                )

    if worst_factor is not None:
        if worst_factor[0] > REANCHOR_TOLERANCE:
            return ReanchorVerdict(True, "factor_mismatch", worst_factor[1])
        return ReanchorVerdict(False, "factor_match", worst_factor[1])
    if worst_close is not None:
        if worst_close[0] > CLOSE_FALLBACK_TOLERANCE:
            return ReanchorVerdict(True, "close_mismatch", worst_close[1])
        return ReanchorVerdict(False, "close_match", worst_close[1])
    # 有重叠行但两侧都取不到可比价格(零价/NaN),同样不能假定尺度一致
    return ReanchorVerdict(
        True, "unverifiable",
        f"重叠日 {len(stored_rows)} 行均无可比价格,无法校验复权尺度")


def _upsert_with_reanchor_check(db: Session, code: str, df: pd.DataFrame,
                                fallback_start: date, end: date) -> int:
    """upsert 前检测前复权重锚,必要时全量回填。

    baostock 前复权价在分红送转后会回溯调整全部历史,只增量更新最近几天
    会让新旧数据处于不同复权尺度,产生假跳空。

    两层检测,权威基准优先:
    1. `audit_scale_against_factors`:库中历史与 quant_adjust_factor 的权威
       因子核对。能发现**既存**的错乱 —— 这是自比对做不到的(两边都从
       close/raw_close 反推,一致地错下去也检测不出);
    2. `detect_reanchor`:库中反推因子 vs 本批反推因子。能发现**本次**增量
       引入的错乱。因子表缺该股数据时(no_factors)这是唯一手段。
    """
    if df.empty:
        return 0

    # 先用权威因子查既存错乱:它为真时本批数据本身可能是对的,
    # 但库中历史已经歪了,必须全量重拉。
    audit = audit_scale_against_factors(db, code)
    verdict = audit if audit.reanchored else detect_reanchor(db, code, df)
    if not verdict.reanchored:
        return upsert_bars(db, code, df)

    history_min = db.execute(
        select(func.min(DailyBar.date)).where(DailyBar.code == code)
    ).scalar()
    start = min(history_min, fallback_start) if history_min else fallback_start
    logger.warning("前复权重锚 %s(%s): %s,强制全量回填自 %s",
                   code, verdict.reason, verdict.detail, start)
    return backfill(db, code, start=start, end=end)


# 公开别名:脚本与 admin API 用这个,别直接用下划线私有名
upsert_with_reanchor_check = _upsert_with_reanchor_check


def safe_backfill(db: Session, code: str, start: date | str,
                  end: date | str | None = None, force: bool = False) -> int:
    """带重锚校验的回填。全市场回填与 admin 手动回填都必须走这里。

    - 库中无该 code 历史,或 `force=True` → 直接全量拉 [start, end];
    - 否则先做重锚判定(比对 close/raw_close 反推的复权系数):
      尺度一致就安全 upsert;尺度不一致或无法校验就从库中最早日期起全量重拉。

    直调 `backfill` 会绕过这层检查,把新尺度 bar 静默接到旧尺度历史上
    (REVIEW §3.1)。
    """
    end_d = end or today_cst()
    start_d = date.fromisoformat(start) if isinstance(start, str) else start
    has_history = db.execute(
        select(func.count()).select_from(DailyBar.__table__)
        .where(DailyBar.__table__.c.code == code)
    ).scalar_one() > 0
    if force or not has_history:
        return backfill(db, code, start_d, end_d)
    df = baostock_client.fetch_daily_bars(code, start_d, end_d)
    return upsert_with_reanchor_check(
        db, code, df, fallback_start=start_d, end=end_d)


def ingest_daily(db: Session, code: str, day: date | None = None,
                 reconcile: bool = True) -> dict:
    """盘后日线增量:baostock 拉最近几天的数据 upsert,
    再用 akshare 对账当日收盘(差异记日志告警)。

    对账只在「本批确实含 day 当日 bar」时进行:节假日/停牌日拿节前旧 bar
    去和 akshare 当日价对账会刷出成批假告警(REVIEW §3.4)。
    """
    day = day or today_cst()
    start = day - timedelta(days=10)  # 多拉几天覆盖节假日/补漏
    df = baostock_client.fetch_daily_bars(code, start, day)
    n = _upsert_with_reanchor_check(db, code, df, fallback_start=start, end=day)

    result: dict = {"code": code, "upserted": n, "reconcile": None,
                    "has_day_bar": False}
    if not df.empty:
        today_rows = df[df["date"] == day]
        result["has_day_bar"] = not today_rows.empty
        if reconcile and not today_rows.empty:
            bs_row = today_rows.iloc[-1]
            try:
                ak_bar = akshare_client.fetch_daily_bar(code, bs_row["date"])
            except Exception as e:  # noqa: BLE001
                logger.warning("akshare 对账查询失败 %s: %s", code, e)
                ak_bar = None
            if ak_bar is not None:
                diff = abs(ak_bar["close"] - float(bs_row["close"]))
                pct = diff / float(bs_row["close"]) * 100 if bs_row["close"] else 0
                rec = {
                    "date": str(bs_row["date"]),
                    "baostock_close": float(bs_row["close"]),
                    "akshare_close": ak_bar["close"],
                    "diff_pct": round(pct, 4),
                }
                result["reconcile"] = rec
                if pct > 1.0:  # 差异超 1% 告警(前复权口径差异属正常,仅提示)
                    logger.warning("日线对账差异 %s %s: %s", code, day, rec)
    return result


def raw_to_qfq(price: float | None, day_factor: float,
               latest_factor: float = 1.0) -> float | None:
    """不复权价换算为前复权价: qfq = raw × 当日因子 ÷ 最新因子。

    **公式待 P0 spike 验证**(scripts/spike_bulk_vs_single.py,
    docs/baostock-bulk-ingest.md §3) —— 验证结论若不一致,只改这一个函数。
    当前假设:
    - 批量日 K 无 adjustflag 入参,返回价视为不复权原始价;
    - 因子与 quant_adjust_factor 同口径(库内 close/raw_close = 当日生效
      前复权因子,见 audit_scale_against_factors)。该口径下前复权锚点因子
      恒为 1,即 latest_factor=1.0,公式退化为 qfq = raw × 当日因子,
      与单票 adjustflag=2 入库的既有前复权行一致。
    """
    if price is None or pd.isna(price):
        return None
    if not day_factor or day_factor <= 0 or not latest_factor or latest_factor <= 0:
        return None
    return float(price) * float(day_factor) / float(latest_factor)


def _chunks(items: list, size: int = 500):
    """IN 查询分块,避免触发 SQLite/MySQL 的占位符上限。"""
    for i in range(0, len(items), size):
        yield items[i:i + size]


def _detect_factor_changes(db: Session, df: pd.DataFrame) -> set[str]:
    """找出因子较库内权威记录发生变化的 code(新除权日 / 上游修订)。

    判定基准是库内该 code divid_operate_date 最大的一条:批量接口返回的是
    当日生效的最新因子记录,比库内最新记录日期更新,或同日期但值偏差超
    REANCHOR_TOLERANCE,即为变化(分红除权/重锚嫌疑)。

    库内完全没有该 code 的因子时无从判定「变化」(可能只是首次同步),
    不算 changed —— 否则首次全量同步会把每只票都送进单票全历史重拉。
    """
    latest: dict[str, tuple[date, float]] = {}
    codes = sorted(df["code"].unique())
    for chunk in _chunks(codes):
        rows = db.execute(
            select(AdjustFactor.code, AdjustFactor.divid_operate_date,
                   AdjustFactor.fore_factor)
            .where(AdjustFactor.code.in_(chunk))
            .order_by(AdjustFactor.code, AdjustFactor.divid_operate_date)
        ).all()
        for code, divid_date, fore in rows:
            latest[code] = (divid_date, float(fore))  # 升序遍历,末条即最新

    changed: set[str] = set()
    for r in df.itertuples():
        base = latest.get(r.code)
        if base is None:
            continue
        divid_date, fore = base
        if r.divid_operate_date > divid_date:
            changed.add(r.code)
        elif (r.divid_operate_date == divid_date and fore > 0
              and abs(float(r.fore_factor) - fore) / fore > REANCHOR_TOLERANCE):
            changed.add(r.code)
    return changed


def sync_adjust_factors_for_day(db: Session, day: date | str) -> dict:
    """按日批量同步全市场复权因子(query_daily_adjust_factor,1 次/日)。

    替代 sync_adjust_factors 的按 code 全表扫(旧函数保留,admin 手动
    触发路径不动)。空响应视为数据源抖动,不清空已有数据(同
    sync_adjust_factors 的保护语义)。

    返回 {"day", "upserted", "empty", "changed"}:changed 是因子较库内
    记录发生变化的 code 列表,供 ingest_market_day 对这些票走
    safe_backfill 单票全历史兜底。
    """
    if isinstance(day, str):
        day = date.fromisoformat(day)
    df = baostock_client.fetch_market_adjust_factors(day)
    if df.empty:
        # 空响应不清空:数据源抖动不该被当成「因子被撤销」
        logger.warning("批量复权因子 %s 返回空,保留库内已有因子", day)
        return {"day": day, "upserted": 0, "empty": True, "changed": []}
    # 变化检测必须在 upsert 之前做,否则基准被自己覆盖
    changed = _detect_factor_changes(db, df)
    upserted = 0
    for code, grp in df.groupby("code"):
        upserted += upsert_adjust_factors(db, code, grp)
    logger.info("批量复权因子 %s: %d 只,写入 %d 行,因子变化 %d 只",
                day, df["code"].nunique(), upserted, len(changed))
    return {"day": day, "upserted": upserted, "empty": False,
            "changed": sorted(changed)}


def _effective_fore_factors(db: Session, codes: list[str],
                            day: date) -> dict[str, float]:
    """各 code 在 day 当日生效的前复权因子(divid_operate_date <= day 的末条)。"""
    effective: dict[str, float] = {}
    for chunk in _chunks(sorted(codes)):
        rows = db.execute(
            select(AdjustFactor.code, AdjustFactor.divid_operate_date,
                   AdjustFactor.fore_factor)
            .where(AdjustFactor.code.in_(chunk),
                   AdjustFactor.divid_operate_date <= day)
            .order_by(AdjustFactor.code, AdjustFactor.divid_operate_date)
        ).all()
        for code, _divid_date, fore in rows:
            effective[code] = float(fore)  # 升序遍历,末条即当日生效值
    return effective


def ingest_market_day(db: Session, day: date | str,
                      codes: set[str] | None = None,
                      backfill_start: date | str = "2015-01-01",
                      sleep_per_reanchor: float = 0.3) -> dict:
    """盘后按日批量入库:全市场日 K 1 次 + 因子 1 次,替代按 code 逐只拉取。

    流程(docs/baostock-bulk-ingest.md §4.1):
    1. `fetch_market_daily_bars` 拉当日全 A 日 K(不复权原始价,口径待
       P0 spike 验证),`sync_adjust_factors_for_day` 拉当日全市场因子并
       upsert 进 quant_adjust_factor(P3 一并完成,不重复请求);
    2. 用库内权威因子按 `raw_to_qfq` 换算前复权 OHLC,**无条件整行
       upsert** 当日行(§7 第 3 条,覆盖盘中可能残留的半根线);
    3. 因子较库内记录发生变化的 code(分红除权/重锚嫌疑)走
       `safe_backfill` 单票全历史重拉,保住重锚防护语义。

    北交所不在批量结果中,自然跳过,仍走原有新浪路径。
    codes 为 None 时写入批量结果的全部 code(历史回填用);盘后增量传
    池内+自选,保持 quant_daily_bar 的收录范围不变。
    """
    if isinstance(day, str):
        day = date.fromisoformat(day)
    if isinstance(backfill_start, str):
        backfill_start = date.fromisoformat(backfill_start)

    with baostock_client.login_session():
        bars = baostock_client.fetch_market_daily_bars(day)
        factor_res = sync_adjust_factors_for_day(db, day)

        result: dict = {
            "day": day, "bars": 0, "codes": 0, "written_codes": [],
            "factors_upserted": factor_res["upserted"],
            "factor_empty": factor_res["empty"],
            "factor_changed": [], "reanchored": [], "failed": [],
        }
        if bars.empty:
            logger.warning("批量日 K %s 返回空(非交易日或数据源异常),未写入", day)
            return result
        if codes is not None:
            bars = bars[bars["code"].isin(codes)]
        if bars.empty:
            return result

        bar_codes = sorted(bars["code"].unique())
        factors = _effective_fore_factors(db, bar_codes, day)
        changed = set(factor_res["changed"])
        result["factor_changed"] = sorted(changed)

        for code, grp in bars.groupby("code"):
            if code in changed:
                # 分红除权/重锚嫌疑:单票全历史重拉,重锚防护语义不丢
                try:
                    n = safe_backfill(db, code, start=backfill_start,
                                      end=day, force=True)
                    result["reanchored"].append(code)
                    result["bars"] += n
                    logger.info("因子变化 %s,已全历史重拉 %d 行", code, n)
                except Exception:  # noqa: BLE001 - 单只失败不影响其他
                    db.rollback()
                    result["failed"].append(code)
                    logger.exception("因子变化 %s 的全历史重拉失败", code)
                if sleep_per_reanchor:
                    time.sleep(sleep_per_reanchor)
                continue
            # 无因子记录 = 从未分红送转,因子为 1,前复权价即原始价
            factor = factors.get(code, 1.0)
            df = pd.DataFrame({
                "date": grp["date"],
                "open": grp["open"].map(lambda v: raw_to_qfq(v, factor)),
                "high": grp["high"].map(lambda v: raw_to_qfq(v, factor)),
                "low": grp["low"].map(lambda v: raw_to_qfq(v, factor)),
                "close": grp["close"].map(lambda v: raw_to_qfq(v, factor)),
                "raw_close": grp["close"],
                "volume": grp["volume"],
                "amount": grp["amount"],
                "is_st": grp["is_st"],
            })
            result["bars"] += upsert_bars(db, code, df)
        # 接口返回的 peTTM/pb/ps 一并入库,避免以后再扫估值源
        try:
            result["valuations"] = upsert_valuations_from_daily_bulk(db, day, bars)
        except Exception:  # noqa: BLE001
            db.rollback()
            result["valuations"] = 0
            logger.exception("批量估值字段落库失败 %s", day)
        result["codes"] = len(bar_codes)
        result["written_codes"] = bar_codes
    logger.info(
        "批量日 K %s: %d 只,写入 %d 行,估值 %d 行,因子变化重拉 %d 只,失败 %d 只",
        day, result["codes"], result["bars"], result.get("valuations", 0),
        len(result["reanchored"]), len(result["failed"]))
    return result



def ingest_snapshot(db: Session, codes: list[str] | None = None) -> int:
    """盘中快照落库。codes 为 None 时落自选股。"""
    df = akshare_client.fetch_spot_snapshot()
    if codes:
        df = df[df["code"].isin(codes)]
    else:
        watch = {r[0] for r in db.execute(
            select(WatchlistItem.code).distinct()).all()}
        df = df[df["code"].isin(watch)]
    rows = [
        {"code": r.code, "ts": r.ts, "price": float(r.price),
         "pct_chg": None if pd.isna(r.pct_chg) else float(r.pct_chg),
         "volume": None if pd.isna(r.volume) else float(r.volume),
         "amount": None if pd.isna(r.amount) else float(r.amount)}
        for r in df.itertuples()
    ]
    if rows:
        db.execute(Snapshot.__table__.insert(), rows)
        db.commit()
    return len(rows)


def cleanup_snapshots(db: Session, retention_days: int) -> int:
    # 快照 ts 按上海时间写入,cutoff 也必须用上海时间,否则非 CST 主机上
    # 保留窗口会整体偏移若干小时
    cutoff = naive_now_cst() - timedelta(days=retention_days)
    res = db.execute(delete(Snapshot).where(Snapshot.ts < cutoff))
    db.commit()
    return res.rowcount or 0


def load_bars_df(db: Session, code: str, start: date | None = None,
                 end: date | None = None,
                 *, extra_fields: list[str] | None = None) -> pd.DataFrame:
    """从库里读日线为 DataFrame(按日期升序),供指标/回测用。

    extra_fields 为 None 时只返回 OHLCV 列,不产生额外查询;传入由
    `required_snapshot_fields` 计算出的估值/财务字段列表时,按 PIT 语义
    left-join 快照表(见 `attach_snapshot_fields`)。
    """
    q = select(DailyBar).where(DailyBar.code == code).order_by(DailyBar.date)
    if start:
        q = q.where(DailyBar.date >= start)
    if end:
        q = q.where(DailyBar.date <= end)
    rows = db.execute(q).scalars().all()
    df = pd.DataFrame(
        [
            {"date": r.date, "open": r.open, "high": r.high, "low": r.low,
             "close": r.close, "raw_close": r.raw_close,
             "volume": r.volume, "amount": r.amount, "is_st": r.is_st}
            for r in rows
        ]
    )
    if extra_fields:
        df = attach_snapshot_fields(db, df, code, extra_fields)
    return df


# load_bars_df 始终提供的日线列(去掉 date),capability 的 available_fields 基数
BAR_FIELDS = frozenset({
    "open", "high", "low", "close", "raw_close", "volume", "amount", "is_st",
})

# StrategySpec 字段 -> 估值快照列(按交易日粒度发布)
VALUATION_SPEC_FIELDS = {
    "pe_ttm": "pe_ttm",
    "pb": "pb",
    "ps_ttm": "ps_ttm",
    "market_cap": "total_market_cap",
}

# StrategySpec 字段 -> 财务快照列(按 report_period + available_date 版本化)
FUNDAMENTAL_SPEC_FIELDS = {
    "roe": "roe",
    "revenue_growth": "revenue_yoy",
    "profit_growth": "profit_yoy",
    "gross_margin": "gross_margin",
    "net_margin": "net_margin",
    "debt_ratio": "debt_ratio",
    "cashflow_quality": "cashflow_ratio",
}

SNAPSHOT_SPEC_FIELDS = {**VALUATION_SPEC_FIELDS, **FUNDAMENTAL_SPEC_FIELDS}


def required_snapshot_fields(spec) -> list[str]:
    """spec.data_requirements 中由快照表供给的必需字段(鸭子类型,不依赖 spec 类)。"""
    return sorted({
        item.field
        for item in getattr(spec, "data_requirements", None) or []
        if getattr(item, "required", True) and item.field in SNAPSHOT_SPEC_FIELDS
    })


def attach_snapshot_fields(db: Session, df: pd.DataFrame, code: str,
                           fields: list[str]) -> pd.DataFrame:
    """把估值/财务快照列按 point-in-time 语义 left-join 到日线帧。

    这是量化系统的生命线:任一交易日只允许看到当日及之前已可用的数据。

    - 估值快照(quant_valuation_snapshot)按交易日发布:对每个交易日取
      `available_date <= 当日` 的最新一条(同 available_date 取 data_date
      最新),且 data_date 不得晚于当日(防御数据口径不一致的脏行);
    - 财务快照(quant_fundamental_snapshot)是报告期的版本化记录:对每个
      交易日取 `available_date <= 当日` 的最新版本(同 available_date 取
      report_period 最新),即「当日收盘时市场已经知道」的财务口径,修订值
      只从其 available_date 起生效。

    没有可用记录的交易日为 NaN(沿用 components.py 的 NaN 传播语义),绝不
    回填 0 或前向填充未发布的值。未知字段名直接报错,避免静默缺列。
    """
    unknown = sorted(set(fields) - set(SNAPSHOT_SPEC_FIELDS))
    if unknown:
        raise ValueError(f"快照表不供给字段: {unknown}")
    requested = list(dict.fromkeys(fields))
    result = df.reset_index(drop=True).copy()
    if result.empty or not requested:
        for field in requested:
            result[field] = float("nan")
        return result

    end = result["date"].max()
    left = pd.DataFrame({"date": pd.to_datetime(result["date"])})
    valuation = {
        field: VALUATION_SPEC_FIELDS[field]
        for field in requested if field in VALUATION_SPEC_FIELDS
    }
    fundamental = {
        field: FUNDAMENTAL_SPEC_FIELDS[field]
        for field in requested if field in FUNDAMENTAL_SPEC_FIELDS
    }
    if valuation:
        rows = db.execute(
            select(ValuationSnapshot).where(
                ValuationSnapshot.code == code,
                ValuationSnapshot.available_date <= end,
            )
        ).scalars().all()
        merged = _asof_join(
            left, rows, valuation,
            key=lambda row: row.available_date,
            tie=lambda row: row.data_date,
        )
        # 防御:data_date 晚于当日的估值行属于口径异常,按无数据置 NaN
        stale = merged["_tie"].notna() & (merged["_tie"] > merged["date"])
        for field in valuation:
            column = merged[f"__{field}"].mask(stale)
            result[field] = pd.to_numeric(column, errors="coerce").to_numpy()
    if fundamental:
        rows = db.execute(
            select(FundamentalSnapshot).where(
                FundamentalSnapshot.code == code,
                FundamentalSnapshot.available_date <= end,
            )
        ).scalars().all()
        merged = _asof_join(
            left, rows, fundamental,
            key=lambda row: row.available_date,
            tie=lambda row: row.report_period,
        )
        for field in fundamental:
            result[field] = pd.to_numeric(
                merged[f"__{field}"], errors="coerce",
            ).to_numpy()
    return result


def _asof_join(left: pd.DataFrame, rows: list, column_map: dict[str, str],
               *, key, tie) -> pd.DataFrame:
    """按 (key, tie) 排序后做 backward as-of 合并:每个交易日取 key <= 当日
    的最后一条,同 key 取 tie 最新。key 是生效日(available_date),保证不读
    未来数据。"""
    records = pd.DataFrame([
        {
            "_key": key(row),
            "_tie": tie(row),
            **{f"__{field}": getattr(row, column)
               for field, column in column_map.items()},
        }
        for row in rows
    ])
    if records.empty:
        empty = left.copy()
        empty["_key"] = pd.NaT
        empty["_tie"] = pd.NaT
        for field in column_map:
            empty[f"__{field}"] = float("nan")
        return empty
    records["_key"] = pd.to_datetime(records["_key"])
    records["_tie"] = pd.to_datetime(records["_tie"])
    records = records.sort_values(["_key", "_tie"], kind="stable")
    return pd.merge_asof(left, records, left_on="date", right_on="_key")


def snapshot_available_fields(db: Session) -> frozenset[str]:
    """库里实际有非空数据的快照字段,供 capability 的 available_fields 使用。

    逐字段一条 limit 1 查询,只在策略保存/启用等低频路径调用。
    """
    available: set[str] = set()
    for field, column in VALUATION_SPEC_FIELDS.items():
        value = db.execute(
            select(getattr(ValuationSnapshot, column)).where(
                getattr(ValuationSnapshot, column).is_not(None),
            ).limit(1)
        ).scalars().first()
        if value is not None:
            available.add(field)
    for field, column in FUNDAMENTAL_SPEC_FIELDS.items():
        value = db.execute(
            select(getattr(FundamentalSnapshot, column)).where(
                getattr(FundamentalSnapshot, column).is_not(None),
            ).limit(1)
        ).scalars().first()
        if value is not None:
            available.add(field)
    return frozenset(available)
