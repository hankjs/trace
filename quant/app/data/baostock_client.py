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
_logged_in = False  # login_session 期间为 True,fetch 复用同一会话


def _login():
    lg = bs.login()
    if lg.error_code != "0":
        raise RuntimeError(f"baostock 登录失败: {lg.error_code} {lg.error_msg}")
    return lg


@contextmanager
def login_session():
    """批量拉取时复用一次登录(baostock 非线程安全,仅在单线程上下文使用)"""
    global _logged_in
    with _login_lock:
        _login()
        _logged_in = True
    try:
        yield
    finally:
        with _login_lock:
            _logged_in = False
            bs.logout()


def fetch_daily_bars(code: str, start: date | str, end: date | str) -> pd.DataFrame:
    """拉取 [start, end] 日线,返回 DataFrame:
    date, open, high, low, close(前复权), raw_close(不复权), volume, amount
    """
    start_s = start.isoformat() if isinstance(start, (date, datetime)) else str(start)
    end_s = end.isoformat() if isinstance(end, (date, datetime)) else str(end)

    fields = "date,open,high,low,close,volume,amount"
    global _logged_in
    own_login = not _logged_in
    with _login_lock:
        if own_login:
            _login()
        try:
            frames = []
            # adjustflag: 2=前复权, 3=不复权
            for adj in ("2", "3"):
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
        finally:
            if own_login:
                bs.logout()

    if frames[0].empty:
        return pd.DataFrame(
            columns=["date", "open", "high", "low", "close", "raw_close", "volume", "amount"]
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

    global _logged_in
    own_login = not _logged_in
    with _login_lock:
        if own_login:
            _login()
        try:
            rs = query(date=day_s)
            if rs.error_code != "0":
                raise RuntimeError(
                    f"baostock 成分股查询失败 {index_name}: {rs.error_code} {rs.error_msg}"
                )
            rows = []
            while (rs.error_code == "0") & rs.next():
                rows.append(rs.get_row_data())
            df = pd.DataFrame(rows, columns=rs.fields)
        finally:
            if own_login:
                bs.logout()

    if df.empty:
        return pd.DataFrame(columns=["code", "name", "update_date"])
    df = df.rename(columns={"code_name": "name", "updateDate": "update_date"})
    return df[["code", "name", "update_date"]]
