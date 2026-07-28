"""批量取最新参考价:盘中快照优先于最近收盘。

供看板 snapshot、持仓估值等热路径复用,避免按 code 两次 N+1 查询。
"""
from __future__ import annotations

from datetime import date, datetime
from typing import Any

from sqlalchemy import and_, func, select
from sqlalchemy.orm import Session

from ..models import DailyBar, Snapshot

# 与 screener 一致,避免超长 IN 列表
_CODE_BATCH = 500


def _batches(codes: list[str]) -> list[list[str]]:
    return [codes[i:i + _CODE_BATCH] for i in range(0, len(codes), _CODE_BATCH)]


def _latest_bars(
    db: Session, codes: list[str],
) -> dict[str, tuple[float, date]]:
    """code -> (close, date)。"""
    if not codes:
        return {}
    out: dict[str, tuple[float, date]] = {}
    for batch in _batches(codes):
        max_dates = (
            select(
                DailyBar.code.label("code"),
                func.max(DailyBar.date).label("max_date"),
            )
            .where(DailyBar.code.in_(batch))
            .group_by(DailyBar.code)
            .subquery()
        )
        rows = db.execute(
            select(DailyBar.code, DailyBar.close, DailyBar.date).join(
                max_dates,
                and_(
                    DailyBar.code == max_dates.c.code,
                    DailyBar.date == max_dates.c.max_date,
                ),
            )
        ).all()
        for code, close, bar_date in rows:
            out[str(code)] = (float(close), bar_date)
    return out


def _latest_snapshots(
    db: Session, codes: list[str],
) -> dict[str, tuple[float, datetime, float | None]]:
    """code -> (price, ts, pct_chg)。"""
    if not codes:
        return {}
    out: dict[str, tuple[float, datetime, float | None]] = {}
    for batch in _batches(codes):
        max_ts = (
            select(
                Snapshot.code.label("code"),
                func.max(Snapshot.ts).label("max_ts"),
            )
            .where(Snapshot.code.in_(batch))
            .group_by(Snapshot.code)
            .subquery()
        )
        rows = db.execute(
            select(Snapshot.code, Snapshot.price, Snapshot.ts, Snapshot.pct_chg).join(
                max_ts,
                and_(
                    Snapshot.code == max_ts.c.code,
                    Snapshot.ts == max_ts.c.max_ts,
                ),
            )
        ).all()
        for code, price, ts, pct_chg in rows:
            out[str(code)] = (
                float(price),
                ts,
                float(pct_chg) if pct_chg is not None else None,
            )
    return out


def latest_reference_prices(
    db: Session, codes: list[str],
) -> dict[str, tuple[float, str]]:
    """每只股票最新参考价:(价格, 来源 snapshot|close)。"""
    quotes = latest_quotes(db, codes)
    return {
        code: (info["price"], info["source"])
        for code, info in quotes.items()
        if info.get("price") is not None and info.get("source")
    }


def latest_quotes(db: Session, codes: list[str]) -> dict[str, dict[str, Any]]:
    """批量最新行情摘要,键为 code。

    字段: price, source(snapshot|close|None), ts(str|None), pct_chg。
    无任何数据时仍返回条目,price/source 为 None。
    """
    unique = list(dict.fromkeys(codes))
    if not unique:
        return {}
    bars = _latest_bars(db, unique)
    snaps = _latest_snapshots(db, unique)
    result: dict[str, dict[str, Any]] = {}
    for code in unique:
        snap = snaps.get(code)
        bar = bars.get(code)
        if snap is not None and (bar is None or snap[1].date() >= bar[1]):
            result[code] = {
                "price": snap[0],
                "source": "snapshot",
                "ts": snap[1].isoformat(sep=" "),
                "pct_chg": snap[2],
            }
        elif bar is not None:
            result[code] = {
                "price": bar[0],
                "source": "close",
                "ts": str(bar[1]),
                "pct_chg": None,
            }
        else:
            result[code] = {
                "price": None,
                "source": None,
                "ts": None,
                "pct_chg": None,
            }
    return result


__all__ = ["latest_quotes", "latest_reference_prices"]
