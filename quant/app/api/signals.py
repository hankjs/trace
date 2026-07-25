"""信号查询。"""
from __future__ import annotations

from datetime import date

from fastapi import APIRouter, Depends, Query
from sqlalchemy import select
from sqlalchemy.orm import Session

from ..auth import require_client, user_id_from_claims
from ..catalog import (render_signal_reason, signal_reason_type,
                       signal_side_name)
from ..db import get_db
from ..models import Signal, Stock, Strategy
from ..strategy.store import visible_to

router = APIRouter(prefix="/api/signals", tags=["signals"])


@router.get("")
def list_signals(date_: date | None = Query(None, alias="date"),
                 code: str | None = None, strategy_id: int | None = None,
                 side: str | None = None,
                 limit: int = Query(200, ge=1, le=1000),
                 db: Session = Depends(get_db),
                 claims: dict = Depends(require_client)):
    """信号列表。

    **按策略可见性过滤**:信号引擎跑所有用户启用的策略,但我只能看到公共策略
    和我自己策略的信号 —— 否则别人的策略名会出现在我的列表里。
    """
    user_id = user_id_from_claims(claims)
    q = (select(Signal, Stock, Strategy)
         .join(Strategy, Strategy.id == Signal.strategy_id)
         .outerjoin(Stock, Stock.code == Signal.code)
         .where(visible_to(user_id))
         .order_by(Signal.date.desc(), Signal.id.desc()).limit(limit))
    if date_:
        q = q.where(Signal.date == date_)
    if code:
        q = q.where(Signal.code == code)
    if strategy_id:
        q = q.where(Signal.strategy_id == strategy_id)
    if side:
        q = q.where(Signal.side == side)
    rows = db.execute(q).all()
    return {
        "count": len(rows),
        "items": [
            {
                "id": signal.id,
                "code": signal.code,
                "name": stock.name if stock else "",
                "industry": stock.industry if stock else "",
                "date": str(signal.date),
                "strategy_id": signal.strategy_id,
                "strategy_name": strategy.name,
                "template": strategy.template,
                "is_system": bool(strategy.is_system),
                "side": signal.side,
                "side_name": signal_side_name(signal.side),
                "price": signal.price,
                "reason": signal.reason,
                "reason_type": signal_reason_type(signal.reason),
                # 措辞按模板选,兜底句用策略实例的名字(用户可能起了自己的名)
                "reason_text": render_signal_reason(
                    strategy.template, signal.side, signal.reason,
                    display_name=strategy.name,
                ),
            }
            for signal, stock, strategy in rows
        ],
    }
