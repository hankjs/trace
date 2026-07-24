"""策略引擎:遍历自选股,对每个策略计算最新信号并落库 quant_signal。

信号定义:目标仓位相对前一交易日发生跳变时产生
  0 -> 1: buy;1 -> 0: sell;不变: 无信号。
"""
from __future__ import annotations

import logging
from datetime import date

from sqlalchemy import select
from sqlalchemy.dialects.mysql import insert as mysql_insert
from sqlalchemy.orm import Session

from ..data.ingest import load_bars_df
from ..models import Signal, Stock
from .strategies import REGISTRY

logger = logging.getLogger(__name__)

# 落库的信号(price 用信号日收盘价)
def _save_signal(db: Session, code: str, day: date, strategy: str,
                 side: str, price: float, reason: dict) -> None:
    stmt = mysql_insert(Signal).values(
        code=code, date=day, strategy=strategy, side=side,
        price=price, reason=reason,
    )
    stmt = stmt.on_duplicate_key_update(price=price, reason=reason)
    db.execute(stmt)


def run_signals(db: Session, day: date | None = None,
                codes: list[str] | None = None,
                strategies: dict[str, dict] | None = None) -> dict:
    """对自选股跑全部注册策略,产出当日信号。

    strategies: {策略名: 参数},None 表示全部注册策略用默认参数。
    返回 {strategy: {code: side}} 汇总。
    """
    day = day or date.today()
    if codes is None:
        codes = [r[0] for r in db.execute(
            select(Stock.code).where(Stock.is_watch.is_(True))).all()]
    if strategies is None:
        strategies = {name: {} for name in REGISTRY}

    summary: dict[str, dict[str, str]] = {}
    for name, params in strategies.items():
        mod = REGISTRY.get(name)
        if mod is None:
            logger.warning("未知策略: %s,跳过", name)
            continue
        summary[name] = {}
        for code in codes:
            df = load_bars_df(db, code, end=day)
            if len(df) < 60:  # 数据太短不足以出可靠信号
                logger.info("%s %s 数据不足(%d 条),跳过", name, code, len(df))
                continue
            pos = mod.positions(df, params)
            if pos.empty or df["date"].iat[-1] != day:
                # 该日无新数据(非交易日或数据未更新),不产生信号
                continue
            cur, prev = int(pos.iat[-1]), int(pos.iat[-2])
            if cur == prev:
                continue
            side = "buy" if cur == 1 else "sell"
            price = float(df["close"].iat[-1])
            reason = {
                "params": params,
                "prev_position": prev,
                "cur_position": cur,
                "close": price,
            }
            _save_signal(db, code, day, name, side, price, reason)
            summary[name][code] = side
            logger.info("信号 %s %s %s @ %.2f", name, code, side, price)
    db.commit()
    return {"date": str(day), "signals": summary,
            "total": sum(len(v) for v in summary.values())}
