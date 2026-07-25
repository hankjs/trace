"""数据入库:历史回填、盘后增量、快照落库、双源对账。"""
from __future__ import annotations

import logging
from datetime import date, datetime, timedelta

import pandas as pd
from sqlalchemy import delete, func, select
from sqlalchemy.dialects.mysql import insert as mysql_insert
from sqlalchemy.dialects.sqlite import insert as sqlite_insert
from sqlalchemy.orm import Session

from ..models import DailyBar, Snapshot, Stock, WatchlistItem
from . import akshare_client, baostock_client, compat  # noqa: F401 - compat 补齐列
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


def upsert_bars(db: Session, code: str, df: pd.DataFrame) -> int:
    """日线 upsert(code+date 唯一键)。

    重锚修复依赖「同 code+date 的旧行被新尺度覆盖」这一语义,所以这里必须
    真正 upsert 而不是 insert-ignore。生产是 MySQL,测试是 SQLite,两种
    方言各用各自的 upsert 语法(否则重锚覆盖行为无法在测试中验证)。
    """
    # 长期停牌股可能返回 OHLC 缺失的行,无法入库,直接丢弃
    df = df.dropna(subset=["close"])
    if df.empty:
        return 0
    rows = [
        {
            "code": code,
            "date": r.date,
            "open": float(r.open),
            "high": float(r.high),
            "low": float(r.low),
            "close": float(r.close),
            "raw_close": None if pd.isna(r.raw_close) else float(r.raw_close),
            "volume": 0.0 if pd.isna(r.volume) else float(r.volume),
            "amount": 0.0 if pd.isna(r.amount) else float(r.amount),
        }
        for r in df.itertuples()
    ]
    updated_cols = ("open", "high", "low", "close", "raw_close", "volume", "amount")
    dialect = db.get_bind().dialect.name
    if dialect == "sqlite":
        stmt = sqlite_insert(DailyBar).values(rows)
        stmt = stmt.on_conflict_do_update(
            index_elements=[DailyBar.code, DailyBar.date],
            set_={c: getattr(stmt.excluded, c) for c in updated_cols},
        )
    else:
        stmt = mysql_insert(DailyBar).values(rows)
        stmt = stmt.on_duplicate_key_update(
            **{c: getattr(stmt.inserted, c) for c in updated_cols}
        )
    db.execute(stmt)
    db.commit()
    return len(rows)


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
    会让新旧数据处于不同复权尺度,产生假跳空。判定见 `detect_reanchor`。
    """
    if df.empty:
        return 0
    verdict = detect_reanchor(db, code, df)
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
                 end: date | None = None) -> pd.DataFrame:
    """从库里读日线为 DataFrame(按日期升序),供指标/回测用。"""
    q = select(DailyBar).where(DailyBar.code == code).order_by(DailyBar.date)
    if start:
        q = q.where(DailyBar.date >= start)
    if end:
        q = q.where(DailyBar.date <= end)
    rows = db.execute(q).scalars().all()
    return pd.DataFrame(
        [
            {"date": r.date, "open": r.open, "high": r.high, "low": r.low,
             "close": r.close, "raw_close": r.raw_close,
             "volume": r.volume, "amount": r.amount}
            for r in rows
        ]
    )
