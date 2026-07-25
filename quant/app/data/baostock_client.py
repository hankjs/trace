"""baostock 客户端:历史回填 + 盘后日线 + 指数成分股。

注意:baostock 是同步阻塞接口,调用方需放到线程池(见 api/admin.py / scheduler.py)。
复权口径:adjustflag="2" 前复权;另查 adjustflag="3" 拿不复权收盘价存入 raw_close。
批量拉取时用 login_session() 包住,避免每次调用都登录/登出。
"""
from __future__ import annotations

import logging
import threading
from contextlib import contextmanager
from datetime import date, datetime

import baostock as bs
import pandas as pd

logger = logging.getLogger(__name__)

_login_lock = threading.Lock()
_login_depth = 0  # 引用计数:>0 表示已有活跃会话,嵌套 login_session 复用同一登录


def _login():
    lg = bs.login()
    if lg.error_code != "0":
        raise RuntimeError(f"baostock 登录失败: {lg.error_code} {lg.error_msg}")
    return lg


def _acquire_session() -> None:
    """引用计数 +1;从 0 到 1 时真正 login。"""
    global _login_depth
    with _login_lock:
        if _login_depth == 0:
            _login()
        _login_depth += 1


def _release_session() -> None:
    """引用计数 -1;归零时才 logout。"""
    global _login_depth
    with _login_lock:
        _login_depth -= 1
        if _login_depth <= 0:
            _login_depth = 0
            bs.logout()


@contextmanager
def login_session():
    """批量拉取时复用一次登录。

    可重入(refcount):嵌套调用只增加引用计数,内层退出不会提前 logout
    把外层剩余工作丢进未登录状态(REVIEW §3.5 baostock 会话不可重入)。
    baostock 本身非线程安全,仍应在单线程上下文批量使用。
    """
    _acquire_session()
    try:
        yield
    finally:
        _release_session()


@contextmanager
def _ensure_session():
    """单次 fetch 用:已有会话则直接复用,否则临时登录一次。

    引用计数的读取与自增在同一把锁内完成,消除原先
    `own_login = not _logged_in` 锁外读取的竞态。锁只保护 login/logout,
    不再横跨整个 fetch,并发的 admin backfill 不会被整段串行阻塞。
    """
    _acquire_session()
    try:
        yield
    finally:
        _release_session()


def fetch_daily_bars(code: str, start: date | str, end: date | str) -> pd.DataFrame:
    """拉取 [start, end] 日线,返回 DataFrame:
    date, open, high, low, close(前复权), raw_close(不复权), volume, amount
    """
    start_s = start.isoformat() if isinstance(start, (date, datetime)) else str(start)
    end_s = end.isoformat() if isinstance(end, (date, datetime)) else str(end)

    # isST 只需从前复权那次请求取:它与复权方式无关,且多取一列不增加请求数
    # (回测的 ST 口径必须逐日,见 alembic 0010 的实测偏差数据)
    fields_adj = "date,open,high,low,close,volume,amount,isST"
    fields_raw = "date,close"
    with _ensure_session():
        frames = []
        # adjustflag: 2=前复权, 3=不复权
        for adj, fields in (("2", fields_adj), ("3", fields_raw)):
            rs = bs.query_history_k_data_plus(
                code, fields, start_date=start_s, end_date=end_s,
                frequency="d", adjustflag=adj,
            )
            if rs.error_code != "0":
                raise RuntimeError(
                    f"baostock 查询失败 {code}: {rs.error_code} {rs.error_msg}"
                )
            rows = []
            while (rs.error_code == "0") & rs.next():
                rows.append(rs.get_row_data())
            df = pd.DataFrame(rows, columns=rs.fields)
            frames.append(df)

    if frames[0].empty:
        return pd.DataFrame(
            columns=["date", "open", "high", "low", "close", "raw_close",
                     "volume", "amount", "is_st"]
        )

    adj = frames[0]
    raw = frames[1][["date", "close"]].rename(columns={"close": "raw_close"}) if not frames[1].empty else None

    df = adj.copy()
    if raw is not None:
        df = df.merge(raw, on="date", how="left")
    else:
        df["raw_close"] = None

    for col in ("open", "high", "low", "close", "raw_close", "volume", "amount"):
        df[col] = pd.to_numeric(df[col], errors="coerce")
    df["date"] = pd.to_datetime(df["date"]).dt.date
    # isST 是 '0'/'1' 字符串;空值保持 None 表示「未知」而非「非 ST」
    if "isST" in df.columns:
        st = pd.to_numeric(df["isST"], errors="coerce")
        df["is_st"] = st.map(lambda v: None if pd.isna(v) else bool(v))
        df = df.drop(columns=["isST"])
    else:
        df["is_st"] = None
    return df


def fetch_index_members(index_name: str,
                        day: date | str | None = None) -> pd.DataFrame:
    """拉取指数成分股。index_name: hs300 / zz500。

    day 为 None 时取最新成分;给定日期时取该日期时点(最近一次调整后)的成分,
    用于重建历史成分区间(无幸存者偏差回测)。
    返回 DataFrame: code(如 sh.600000), name, update_date
    """
    query = {
        "hs300": bs.query_hs300_stocks,
        "zz500": bs.query_zz500_stocks,
    }.get(index_name)
    if query is None:
        raise ValueError(f"未知指数: {index_name},可选: hs300 / zz500")

    day_s = day.isoformat() if isinstance(day, (date, datetime)) else (day or "")

    with _ensure_session():
        rs = query(date=day_s)
        if rs.error_code != "0":
            raise RuntimeError(
                f"baostock 成分股查询失败 {index_name}: {rs.error_code} {rs.error_msg}"
            )
        rows = []
        while (rs.error_code == "0") & rs.next():
            rows.append(rs.get_row_data())
        df = pd.DataFrame(rows, columns=rs.fields)

    if df.empty:
        return pd.DataFrame(columns=["code", "name", "update_date"])
    df = df.rename(columns={"code_name": "name", "updateDate": "update_date"})
    return df[["code", "name", "update_date"]]


def fetch_trade_dates(start: date | str, end: date | str) -> pd.DataFrame:
    """拉取 [start, end] 的交易日历。

    返回 DataFrame: date(datetime.date), is_open(bool)
    baostock query_trade_dates 返回 calendar_date / is_trading_day('1'/'0')。
    """
    start_s = start.isoformat() if isinstance(start, (date, datetime)) else str(start)
    end_s = end.isoformat() if isinstance(end, (date, datetime)) else str(end)

    with _ensure_session():
        rs = bs.query_trade_dates(start_date=start_s, end_date=end_s)
        if rs.error_code != "0":
            raise RuntimeError(
                f"baostock 交易日历查询失败: {rs.error_code} {rs.error_msg}"
            )
        rows = []
        while (rs.error_code == "0") & rs.next():
            rows.append(rs.get_row_data())
        df = pd.DataFrame(rows, columns=rs.fields)

    if df.empty:
        return pd.DataFrame(columns=["date", "is_open"])
    df = df.rename(columns={"calendar_date": "date"})
    df["date"] = pd.to_datetime(df["date"]).dt.date
    df["is_open"] = df["is_trading_day"].astype(str).str.strip() == "1"
    return df[["date", "is_open"]]


def fetch_stock_basic() -> pd.DataFrame:
    """拉取全市场证券基础资料(含上市/退市日期与证券类型)。

    返回 DataFrame: code, name, list_date(date|None), delist_date(date|None),
    type(str: 1股票/2指数/3其他/4可转债/5ETF), status(str: 1上市/0退市)
    """
    with _ensure_session():
        rs = bs.query_stock_basic()
        if rs.error_code != "0":
            raise RuntimeError(
                f"baostock 证券资料查询失败: {rs.error_code} {rs.error_msg}"
            )
        rows = []
        while (rs.error_code == "0") & rs.next():
            rows.append(rs.get_row_data())
        df = pd.DataFrame(rows, columns=rs.fields)

    cols = ["code", "name", "list_date", "delist_date", "type", "status"]
    if df.empty:
        return pd.DataFrame(columns=cols)
    df = df.rename(columns={"code_name": "name", "ipoDate": "list_date",
                            "outDate": "delist_date"})
    for col in ("list_date", "delist_date"):
        if col not in df.columns:
            df[col] = None
        df[col] = pd.to_datetime(df[col], errors="coerce").dt.date
    for col in ("type", "status"):
        df[col] = df[col].astype(str).str.strip() if col in df.columns else ""
    return df[cols]


def fetch_adjust_factors(code: str, start: date | str | None = None,
                         end: date | str | None = None) -> pd.DataFrame:
    """拉取复权因子(权威值,按除权日稀疏返回)。

    baostock query_adjust_factor 返回 code / dividOperateDate /
    foreAdjustFactor / backAdjustFactor / adjustFactor。

    为什么要采权威值而不是从库里 close/raw_close 反推:反推只能反推出库里
    **已有**的数据,若某股历史本身已错乱,反推的因子会连同错误一起继承,
    拿它当重锚检测基准就是循环论证(见 alembic 0007 的 docstring)。

    返回 DataFrame: code, divid_operate_date(date), fore_factor, back_factor
    """
    start_s = start.isoformat() if isinstance(start, (date, datetime)) else (start or "")
    end_s = end.isoformat() if isinstance(end, (date, datetime)) else (end or "")

    with _ensure_session():
        rs = bs.query_adjust_factor(code=code, start_date=start_s, end_date=end_s)
        if rs.error_code != "0":
            raise RuntimeError(
                f"baostock 复权因子查询失败 {code}: {rs.error_code} {rs.error_msg}"
            )
        rows = []
        while (rs.error_code == "0") & rs.next():
            rows.append(rs.get_row_data())
        df = pd.DataFrame(rows, columns=rs.fields)

    cols = ["code", "divid_operate_date", "fore_factor", "back_factor"]
    if df.empty:
        return pd.DataFrame(columns=cols)
    df = df.rename(columns={"dividOperateDate": "divid_operate_date",
                            "foreAdjustFactor": "fore_factor",
                            "backAdjustFactor": "back_factor"})
    df["divid_operate_date"] = pd.to_datetime(
        df["divid_operate_date"], errors="coerce").dt.date
    for col in ("fore_factor", "back_factor"):
        if col not in df.columns:
            df[col] = None
        df[col] = pd.to_numeric(df[col], errors="coerce")
    # 除权日或前因子缺失的行无法使用
    df = df.dropna(subset=["divid_operate_date", "fore_factor"])
    return df[cols]
