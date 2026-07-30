"""手工成交记账。"""
from __future__ import annotations

from datetime import date

from sqlalchemy import select
from sqlalchemy.orm import Session

from ..models import Trade

QTY_EPS = 1e-9  # 浮点持仓比较容差


class OversellError(ValueError):
    """卖出数量超过该时点持仓,会造成负持仓。"""


def _assert_no_negative_position(trades: list[Trade]) -> None:
    """校验整条时间序列:任一时点该标的的持仓都不得为负。

    不能只看"当前持仓够不够":补录一笔历史卖出会让它之后的成交重新排序,
    中间某个时点可能已经透支。因此按 (trade_date, id) 重放整条序列。
    """
    running = 0.0
    for t in trades:
        if t.side == "buy":
            running += t.qty
            continue
        if t.qty > running + QTY_EPS:
            raise OversellError(
                f"{t.code} 在 {t.trade_date} 卖出 {t.qty:g} 股,"
                f"但该时点仅持有 {running:g} 股,会造成负持仓"
            )
        running -= t.qty


def _dialect_name(db: Session) -> str:
    bind = db.get_bind()
    return bind.dialect.name if bind is not None else "unknown"


def _lock_trades_for_update(db: Session, user_id: int, code: str) -> None:
    """MySQL 下对已有成交行加 SELECT ... FOR UPDATE,防并发同标超卖。

    SQLite 等不支持行级锁的方言直接跳过:单连接/文件库的事务隔离已能
    满足测试场景,生产互斥由 MySQL 的 next-key 锁保证。
    """
    if _dialect_name(db) != "mysql":
        return
    db.execute(
        select(Trade.id)
        .where(Trade.user_id == user_id, Trade.code == code)
        .order_by(Trade.id)
        .with_for_update()
    ).all()


def _trades_for_check(db: Session, user_id: int, code: str) -> list[Trade]:
    """取该用户该标的的全部成交,按 (日期, id) 升序——重放校验的口径。"""
    return list(db.execute(
        select(Trade).where(Trade.user_id == user_id, Trade.code == code)
        .order_by(Trade.trade_date, Trade.id)
    ).scalars().all())


def add_trade(db: Session, user_id: int, code: str, trade_date: date, side: str,
              price: float, qty: float, fee: float = 0.0,
              note: str = "") -> Trade:
    if side not in ("buy", "sell"):
        raise ValueError("side 必须是 buy 或 sell")
    if price <= 0 or qty <= 0:
        raise ValueError("price / qty 必须为正数")
    if fee < 0:
        raise ValueError("fee 不能为负")
    t = Trade(user_id=user_id, code=code, trade_date=trade_date, side=side, price=price,
              qty=qty, fee=fee, note=note)
    db.add(t)
    # 在同一事务内 flush 后连同已有成交重放校验:超卖直接回滚拒绝。
    # 此前 positions.py 用 min(卖出量, 持仓量) 静默截断,库里留着 200 股的
    # 成交、盈亏只算 100 股,两边永久不一致;对零持仓的卖出更是凭空消失。
    # MySQL 下先加 FOR UPDATE,用 next-key 锁把同账户同标的的并发写入串行化。
    try:
        _lock_trades_for_update(db, user_id, code)
        db.flush()
        _assert_no_negative_position(_trades_for_check(db, user_id, code))
    except Exception:
        db.rollback()
        raise
    db.commit()
    db.refresh(t)
    return t


def list_trades(db: Session, user_id: int, code: str | None = None) -> list[Trade]:
    q = (select(Trade).where(Trade.user_id == user_id)
         .order_by(Trade.trade_date, Trade.id))
    if code:
        q = q.where(Trade.code == code)
    return list(db.execute(q).scalars().all())


def delete_trade(db: Session, user_id: int, trade_id: int) -> bool:
    t = db.execute(select(Trade).where(
        Trade.id == trade_id, Trade.user_id == user_id,
    )).scalar_one_or_none()
    if t is None:
        return False
    code = t.code
    db.delete(t)
    # 删掉一笔买入会让它之后的卖出失去支撑,同样要校验整条序列
    try:
        _lock_trades_for_update(db, user_id, code)
        db.flush()
        _assert_no_negative_position(_trades_for_check(db, user_id, code))
    except Exception:
        db.rollback()
        raise
    db.commit()
    return True
