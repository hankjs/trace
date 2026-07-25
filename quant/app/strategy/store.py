"""策略行的查询与可见性规则。

收成一个模块而不是散落在各处:可见性判断漏写一次就是越权读取或漏数据 ——
`quant_pool` 的历史教训见 alembic 0011 的文档字符串。定时任务(信号引擎、批量
评估)、回测 API 和策略 CRUD 都走这里的同一份判断。
"""
from __future__ import annotations

from sqlalchemy import func, or_, select
from sqlalchemy.orm import Session

from ..models import SYSTEM_OWNER_ID, Strategy

# 每用户的策略数上限。夜间信号引擎的成本是「股票数 × 启用策略数」,
# 启用数单独设一个更紧的上限:存着不跑的策略只占一行,跑起来的每一个都要
# 乘进全市场股票数。
MAX_STRATEGIES_PER_USER = 50
MAX_ENABLED_PER_USER = 10


def visible_to(user_id: str):
    """可见性条件:公共策略 OR 我的策略。

    与 `api/pools.visible_to` 同构,但没有 grant 分支 —— 策略当前只有「公共」
    和「我的」两档,定向分享等真有需求再照 `quant_pool_grant` 补。
    """
    return or_(Strategy.is_system.is_(True), Strategy.owner_id == user_id)


def can_edit(strategy: Strategy, user_id: str) -> bool:
    """可写:自己的策略。公共策略一律只读(改一次影响所有用户)。"""
    if strategy.is_system:
        return False
    return strategy.owner_id == user_id


def list_visible(db: Session, user_id: str) -> list[Strategy]:
    """我能看到的全部策略,公共的排在前面(与池列表同口径)。"""
    return list(db.execute(
        select(Strategy).where(visible_to(user_id))
        .order_by(Strategy.is_system.desc(), Strategy.id)
    ).scalars().all())


def enabled_strategies(db: Session, kind: str | None = None) -> list[Strategy]:
    """全部启用的策略(**跨用户**),供定时任务使用。

    定时任务不属于任何用户,要跑所有人启用的策略,故这里刻意不过滤 owner。
    调用方按 `strategy.owner_id` 归属产出的信号。
    """
    q = select(Strategy).where(Strategy.enabled.is_(True))
    if kind is not None:
        q = q.where(Strategy.kind == kind)
    return list(db.execute(q.order_by(Strategy.id)).scalars().all())


def system_strategies(db: Session) -> list[Strategy]:
    return list(db.execute(
        select(Strategy).where(Strategy.is_system.is_(True))
        .order_by(Strategy.id)
    ).scalars().all())


def count_owned(db: Session, user_id: str, enabled_only: bool = False) -> int:
    """用户自建策略数(公共策略不计入配额)。"""
    q = select(func.count()).select_from(Strategy).where(
        Strategy.owner_id == user_id, Strategy.is_system.is_(False))
    if enabled_only:
        q = q.where(Strategy.enabled.is_(True))
    return int(db.execute(q).scalar() or 0)


def default_strategy(db: Session, kind: str = "single") -> Strategy | None:
    """缺省策略:编号最小的公共策略,供前端首次进入页面时预选。"""
    return db.execute(
        select(Strategy).where(
            Strategy.is_system.is_(True), Strategy.kind == kind)
        .order_by(Strategy.id)
    ).scalars().first()


__all__ = [
    "MAX_ENABLED_PER_USER",
    "MAX_STRATEGIES_PER_USER",
    "SYSTEM_OWNER_ID",
    "can_edit",
    "count_owned",
    "default_strategy",
    "enabled_strategies",
    "list_visible",
    "system_strategies",
    "visible_to",
]
