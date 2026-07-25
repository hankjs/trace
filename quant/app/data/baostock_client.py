"""baostock 客户端:历史回填 + 盘后日线 + 指数成分股。

注意:baostock 是同步阻塞接口,调用方需放到线程池(见 api/admin.py / scheduler.py)。
复权口径:adjustflag="2" 前复权;另查 adjustflag="3" 拿不复权收盘价存入 raw_close。
批量拉取时用 login_session() 包住,避免每次调用都登录/登出。
"""
from __future__ import annotations

import logging
import threading
from collections.abc import Callable
from contextlib import contextmanager
from datetime import date, datetime
from typing import Any

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


class BaoStockClient:
    """管理 baostock 会话和非线程安全的响应读取。"""

    def __init__(self) -> None:
        self._query_lock = threading.RLock()

    @contextmanager
    def session(self):
        _acquire_session()
        try:
            yield
        finally:
            _release_session()

    def _query_frame(
        self,
        query: Callable[[], Any],
        error_context: str,
    ) -> pd.DataFrame:
        """执行查询、检查错误并完整展开结果集。"""
        with self.session(), self._query_lock:
            result = query()
            if result.error_code != "0":
                raise RuntimeError(
                    f"{error_context}: {result.error_code} {result.error_msg}"
                )
            rows = []
            while result.error_code == "0" and result.next():
                rows.append(result.get_row_data())
            if result.error_code != "0":
                raise RuntimeError(
                    f"{error_context}: {result.error_code} {result.error_msg}"
                )
            return pd.DataFrame(rows, columns=result.fields)


_client = BaoStockClient()


@contextmanager
def login_session():
    """批量拉取时复用一次登录。

    可重入(refcount):嵌套调用只增加引用计数,内层退出不会提前 logout
    把外层剩余工作丢进未登录状态(REVIEW §3.5 baostock 会话不可重入)。
    baostock 本身非线程安全,查询和响应读取由客户端锁串行化。
    """
    with _client.session():
        yield


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
    with _client.session():
        frames = []
        # adjustflag: 2=前复权, 3=不复权
        for adj, fields in (("2", fields_adj), ("3", fields_raw)):
            frames.append(_client._query_frame(
                lambda adj=adj, fields=fields: bs.query_history_k_data_plus(
                    code, fields, start_date=start_s, end_date=end_s,
                    frequency="d", adjustflag=adj,
                ),
                f"baostock 查询失败 {code}",
            ))

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

    df = _client._query_frame(
        lambda: query(date=day_s),
        f"baostock 成分股查询失败 {index_name}",
    )

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

    df = _client._query_frame(
        lambda: bs.query_trade_dates(start_date=start_s, end_date=end_s),
        "baostock 交易日历查询失败",
    )

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
    df = _client._query_frame(
        bs.query_stock_basic,
        "baostock 证券资料查询失败",
    )

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

    df = _client._query_frame(
        lambda: bs.query_adjust_factor(
            code=code, start_date=start_s, end_date=end_s),
        f"baostock 复权因子查询失败 {code}",
    )

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
