"""akshare 客户端:股票列表、盘中快照、日线对账。

网络偶发失败是常态:所有对外函数自带重试,单次失败抛异常由调用方记录,
绝不能影响服务整体运行。
"""
from __future__ import annotations

import logging
import time
from datetime import date

import akshare as ak
import pandas as pd

from .clock import naive_now_cst

logger = logging.getLogger(__name__)


def _retry(fn, retries: int = 3, delay: float = 2.0):
    last: Exception | None = None
    for i in range(retries):
        try:
            return fn()
        except Exception as e:  # noqa: BLE001 - 数据源网络异常类型不可控
            last = e
            logger.warning("akshare 调用失败(第 %d/%d 次): %s", i + 1, retries, e)
            if i < retries - 1:
                time.sleep(delay)
    raise last  # type: ignore[misc]


def code_to_symbol(code: str) -> str:
    """sh.600519 -> 600519(akshare 用的 6 位代码)"""
    return code.split(".")[-1]


def symbol_to_code(symbol: str) -> str:
    """600519 -> sh.600519 / 000001 -> sz.000001"""
    if symbol.startswith(("4", "8", "92")):
        return f"bj.{symbol}"
    if symbol.startswith(("6", "9")):
        return f"sh.{symbol}"
    return f"sz.{symbol}"


def fetch_stock_list() -> pd.DataFrame:
    """A股代码+名称列表,返回 DataFrame: code(sh.600519 格式), name"""
    df = _retry(lambda: ak.stock_info_a_code_name())
    df = df.rename(columns={"code": "symbol", "name": "name"})
    df["code"] = df["symbol"].astype(str).map(symbol_to_code)
    return df[["code", "name"]]


def _spot_em() -> pd.DataFrame:
    """东财全市场快照"""
    df = ak.stock_zh_a_spot_em()
    return pd.DataFrame(
        {
            "code": df["代码"].astype(str).map(symbol_to_code),
            "price": pd.to_numeric(df["最新价"], errors="coerce"),
            "pct_chg": pd.to_numeric(df["涨跌幅"], errors="coerce"),
            "volume": pd.to_numeric(df["成交量"], errors="coerce"),
            "amount": pd.to_numeric(df["成交额"], errors="coerce"),
        }
    )


def _spot_sina() -> pd.DataFrame:
    """新浪全市场快照(降级源,较慢,约 20~30 秒)。代码形如 sh600519。"""
    df = ak.stock_zh_a_spot()
    codes = df["代码"].astype(str)
    return pd.DataFrame(
        {
            "code": codes.str[:2] + "." + codes.str[2:],
            "price": pd.to_numeric(df["最新价"], errors="coerce"),
            "pct_chg": pd.to_numeric(df["涨跌幅"], errors="coerce"),
            "volume": pd.to_numeric(df["成交量"], errors="coerce"),
            "amount": pd.to_numeric(df["成交额"], errors="coerce"),
        }
    )


def fetch_spot_snapshot() -> pd.DataFrame:
    """全市场实时快照:东财优先,失败自动降级新浪。返回 DataFrame:
    code, ts, price, pct_chg, volume, amount
    """
    try:
        out = _retry(_spot_em, retries=2, delay=1.5)
    except Exception:
        logger.warning("东财快照不可用,降级为新浪源")
        out = _retry(_spot_sina, retries=2, delay=2.0)
    out["ts"] = naive_now_cst().replace(microsecond=0)
    return out.dropna(subset=["price"])


def _daily_bar_em(code: str, day: date) -> dict | None:
    symbol = code_to_symbol(code)
    d = day.strftime("%Y%m%d")
    df = ak.stock_zh_a_hist(
        symbol=symbol, period="daily", start_date=d, end_date=d, adjust="qfq"
    )
    if df is None or df.empty:
        return None
    row = df.iloc[-1]
    return {
        "date": day,
        "open": float(row["开盘"]),
        "high": float(row["最高"]),
        "low": float(row["最低"]),
        "close": float(row["收盘"]),
        "volume": float(row["成交量"]),
        "amount": float(row["成交额"]),
    }


def _daily_bar_sina(code: str, day: date) -> dict | None:
    symbol = code.replace(".", "")  # sh.600519 -> sh600519
    d = day.strftime("%Y%m%d")
    df = ak.stock_zh_a_daily(symbol=symbol, start_date=d, end_date=d, adjust="qfq")
    if df is None or df.empty:
        return None
    row = df.iloc[-1]
    return {
        "date": day,
        "open": float(row["open"]),
        "high": float(row["high"]),
        "low": float(row["low"]),
        "close": float(row["close"]),
        "volume": float(row["volume"]),
        "amount": float(row["amount"]),
    }


def fetch_daily_bar(code: str, day: date) -> dict | None:
    """单日日线(前复权),用于盘后双源对账:东财优先,失败降级新浪。"""
    try:
        return _retry(lambda: _daily_bar_em(code, day), retries=2, delay=1.5)
    except Exception:
        logger.warning("东财日线不可用 %s,降级为新浪源", code)
        return _retry(lambda: _daily_bar_sina(code, day), retries=2, delay=1.5)


def fetch_bj_daily_bars(code: str, start: date, end: date) -> pd.DataFrame:
    """北交所日线(新浪源),同时返回前复权价与不复权收盘价。

    baostock 完全不覆盖北交所(`bj.` 前缀报 10004011,换 `sh.`/`sz.` 前缀
    参数校验虽通过但返回 0 行),故这批标的只能走 akshare。见 alembic 0008。

    新浪源同时提供 adjust='' 与 adjust='qfq',两者相除即得复权因子;实测
    末行因子为 1.0,说明与 baostock 同为「最新日为基准的前复权」口径,
    两个来源的数据放同一张 quant_daily_bar 不会混口径。

    返回列与 baostock_client.fetch_daily_bars 一致:
    date, open, high, low, close(前复权), raw_close(不复权), volume, amount
    """
    symbol = code.replace(".", "")          # bj.920000 -> bj920000
    s, e = start.strftime("%Y%m%d"), end.strftime("%Y%m%d")
    cols = ["date", "open", "high", "low", "close", "raw_close",
            "volume", "amount"]

    qfq = _retry(lambda: ak.stock_zh_a_daily(
        symbol=symbol, start_date=s, end_date=e, adjust="qfq"))
    raw = _retry(lambda: ak.stock_zh_a_daily(
        symbol=symbol, start_date=s, end_date=e, adjust=""))
    if qfq is None or qfq.empty:
        return pd.DataFrame(columns=cols)

    qfq = qfq.copy()
    qfq["date"] = pd.to_datetime(qfq["date"]).dt.date
    out = qfq[["date", "open", "high", "low", "close", "volume", "amount"]].copy()

    if raw is not None and not raw.empty:
        raw = raw.copy()
        raw["date"] = pd.to_datetime(raw["date"]).dt.date
        out = out.merge(
            raw[["date", "close"]].rename(columns={"close": "raw_close"}),
            on="date", how="left")
    else:
        # 拿不到不复权价时留空:下游重锚检测会退化为 close 比对,
        # 但不能假造 raw_close(那会让因子计算出错)
        out["raw_close"] = None

    return out[cols]
