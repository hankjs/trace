"""动态因子定义缓存与目录字段生成。"""
from __future__ import annotations

import threading
import time
from dataclasses import dataclass
from typing import Any

from sqlalchemy import select
from sqlalchemy.orm import Session

from ..models import FactorDef

_CACHE_TTL_SECONDS = 60

_cache: dict[str, "FactorDefSnapshot"] = {}
_cache_all: dict[str, "FactorDefSnapshot"] = {}
_cache_at: float = 0.0
_cache_lock = threading.Lock()


@dataclass(frozen=True, slots=True)
class FactorDefSnapshot:
    """FactorDef 的会话无关快照,避免跨 Session 缓存 ORM 对象。"""

    id: int
    key: str
    name: str
    description: str
    category: str
    unit: str | None
    direction: str
    limits: str
    value_type: str
    input_scale: float | None
    expression: dict
    expression_hash: str
    min_bars: int
    enabled: bool
    is_system: bool
    created_at: Any = None
    updated_at: Any = None


def _snapshot(row: FactorDef) -> FactorDefSnapshot:
    return FactorDefSnapshot(
        id=row.id,
        key=row.key,
        name=row.name,
        description=row.description or "",
        category=row.category or "",
        unit=row.unit,
        direction=row.direction or "",
        limits=row.limits or "",
        value_type=row.value_type or "number",
        input_scale=row.input_scale,
        expression=dict(row.expression or {}),
        expression_hash=row.expression_hash or "",
        min_bars=row.min_bars or 1,
        enabled=bool(row.enabled),
        is_system=bool(row.is_system),
        created_at=row.created_at,
        updated_at=row.updated_at,
    )


def _now() -> float:
    return time.monotonic()


def _load_defs(db: Session, *, enabled_only: bool) -> dict[str, FactorDefSnapshot]:
    q = select(FactorDef)
    if enabled_only:
        q = q.where(FactorDef.enabled.is_(True))
    return {row.key: _snapshot(row) for row in db.execute(q).scalars()}


def _refresh_if_stale(db: Session) -> None:
    global _cache, _cache_all, _cache_at
    now = _now()
    if now - _cache_at < _CACHE_TTL_SECONDS:
        return
    with _cache_lock:
        # 双检锁
        if now - _cache_at < _CACHE_TTL_SECONDS:
            return
        _cache = _load_defs(db, enabled_only=True)
        _cache_all = _load_defs(db, enabled_only=False)
        _cache_at = _now()


def invalidate_factor_cache() -> None:
    """使进程内因子定义缓存失效。"""
    global _cache_at
    with _cache_lock:
        _cache_at = 0.0


def load_enabled_defs(db: Session) -> list[FactorDefSnapshot]:
    """返回当前启用的因子定义列表(带 60s 进程缓存)。"""
    _refresh_if_stale(db)
    with _cache_lock:
        return list(_cache.values())


def load_all_defs(db: Session) -> list[FactorDefSnapshot]:
    """返回全部因子定义列表(含禁用,带 60s 进程缓存)。"""
    _refresh_if_stale(db)
    with _cache_lock:
        return list(_cache_all.values())


def _number_operators() -> tuple[str, ...]:
    return (
        "eq", "ne", "gt", "gte", "lt", "lte",
        "between", "is_null", "not_null",
    )


def _boolean_operators() -> tuple[str, ...]:
    return ("eq", "ne", "is_null", "not_null")


def _default_input_scale(unit: str | None, value_type: str,
                         explicit: float | None) -> float | None:
    if explicit is not None:
        return explicit
    if value_type != "number":
        return None
    return 0.01 if unit == "%" else 1.0


def factor_catalog_fields(db: Session) -> dict[str, dict[str, Any]]:
    """把启用的 FactorDef 转成 catalog._field 形状的目录条目。

    只包含 enabled=True 的定义,供筛选目录与前端因子目录使用。
    """
    defs = load_enabled_defs(db)
    result: dict[str, dict[str, Any]] = {}
    for d in defs:
        value_type = d.value_type or "number"
        operators = _boolean_operators() if value_type == "boolean" else _number_operators()
        result[d.key] = {
            "key": d.key,
            "name": d.name,
            "description": d.description or "",
            "category": d.category or "",
            "unit": d.unit,
            "direction": d.direction or "",
            "limits": d.limits or "",
            "value_type": value_type,
            "input_scale": _default_input_scale(d.unit, value_type, d.input_scale),
            "operators": list(operators),
            "source": "technical",
            "available": True,
        }
    return result


__all__ = [
    "FactorDefSnapshot",
    "factor_catalog_fields",
    "invalidate_factor_cache",
    "load_all_defs",
    "load_enabled_defs",
]
