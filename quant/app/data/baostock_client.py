"""baostock 客户端:历史回填 + 盘后日线。

注意:baostock 是同步阻塞接口,调用方需放到线程池(见 api/admin.py / scheduler.py)。
复权口径:adjustflag="2" 前复权;另查 adjustflag="3" 拿不复权收盘价存入 raw_close。
"""
from __future__ import annotations

import logging
import threading
from datetime import date, datetime

import baostock as bs
import pandas as pd

logger = logging.getLogger(__name__)

_login_lock = threading.Lock()


def _login():
    lg = bs.login()
    if lg.error_code != "0":
        raise RuntimeError(f"baostock 登录失败: {lg.error_code} {lg.error_msg}")
    return lg


def fetch_daily_bars(code: str, start: date | str, end: date | str) -> pd.DataFrame:
    """拉取 [start, end] 日线,返回 DataFrame:
    date, open, high, low, close(前复权), raw_close(不复权), volume, amount
    """
    start_s = start.isoformat() if isinstance(start, (date, datetime)) else str(start)
    end_s = end.isoformat() if isinstance(end, (date, datetime)) else str(end)

    fields = "date,open,high,low,close,volume,amount"
    with _login_lock:
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
