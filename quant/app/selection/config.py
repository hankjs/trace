"""选股流水线配置加载、校验与缓存。"""
from __future__ import annotations

import math
import threading
import time
from dataclasses import dataclass
from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field, model_validator
from sqlalchemy import select
from sqlalchemy.orm import Session

from ..models import SelectionConfig
from ..factors import load_enabled_defs

_CACHE_TTL_SECONDS = 60

_config_cache: "SelectionConfigSnapshot | None" = None
_config_at: float = 0.0
_config_lock = threading.Lock()


class StrictModel(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)


HardFilterType = Literal[
    "exclude_st", "exclude_suspended", "min_bars",
    "factor_gte", "factor_lte", "factor_gt", "factor_lt",
    "row_flag",
]


class ExcludeStFilter(StrictModel):
    type: Literal["exclude_st"]


class ExcludeSuspendedFilter(StrictModel):
    type: Literal["exclude_suspended"]


class MinBarsFilter(StrictModel):
    type: Literal["min_bars"]
    value: int = Field(..., ge=1)


class FactorFilter(StrictModel):
    type: Literal["factor_gte", "factor_lte", "factor_gt", "factor_lt"]
    factor: str = Field(..., min_length=1)
    value: float = Field(...)

    @model_validator(mode="after")
    def finite_value(self) -> "FactorFilter":
        if not math.isfinite(self.value):
            raise ValueError("factor 过滤值必须是有限数字")
        return self


class RowFlagFilter(StrictModel):
    type: Literal["row_flag"]
    field: Literal["above_ma20"]
    value: bool


HardFilter = (
    ExcludeStFilter | ExcludeSuspendedFilter | MinBarsFilter |
    FactorFilter | RowFlagFilter
)


class VolConfirmConfig(StrictModel):
    factor: str = Field(..., min_length=1)
    cap: float = Field(..., ge=0)
    weight: float = Field(..., ge=0)

    @model_validator(mode="after")
    def finite_numbers(self) -> "VolConfirmConfig":
        if not math.isfinite(self.cap) or not math.isfinite(self.weight):
            raise ValueError("cap 与 weight 必须是有限数字")
        return self


class SelectionConfigUpdateIn(StrictModel):
    name: str | None = Field(None, min_length=1, max_length=64)
    score_weights: dict[str, float]
    vol_confirm: VolConfirmConfig
    hard_filters: list[HardFilter]
    top_n: int = Field(..., ge=1, le=100)


@dataclass(frozen=True, slots=True)
class SelectionConfigSnapshot:
    """SelectionConfig 的会话无关快照,避免跨 Session 缓存 ORM 对象。"""

    id: int
    name: str
    is_active: bool
    score_weights: dict[str, float]
    vol_confirm: dict[str, Any]
    hard_filters: list[dict[str, Any]]
    top_n: int
    updated_at: Any = None


def _snapshot(row: SelectionConfig) -> SelectionConfigSnapshot:
    return SelectionConfigSnapshot(
        id=row.id,
        name=row.name or "",
        is_active=bool(row.is_active),
        score_weights=dict(row.score_weights or {}),
        vol_confirm=dict(row.vol_confirm or {}),
        hard_filters=list(row.hard_filters or []),
        top_n=row.top_n or 30,
        updated_at=row.updated_at,
    )


def _now() -> float:
    return time.monotonic()


def invalidate_selection_config_cache() -> None:
    """使进程内选股配置缓存失效。"""
    global _config_at
    with _config_lock:
        _config_at = 0.0


def get_active_selection_config_row(db: Session) -> SelectionConfig:
    """返回当前 active 的选股配置 ORM 行;无配置时抛出 RuntimeError。"""
    row = db.execute(
        select(SelectionConfig).where(SelectionConfig.is_active.is_(True))
        .order_by(SelectionConfig.id.desc())
        .limit(1)
    ).scalar_one_or_none()
    if row is None:
        raise RuntimeError(
            "缺少 active 的选股配置(quant_selection_config),"
            "请先执行 alembic upgrade head 或 seed 默认配置"
        )
    return row


def load_selection_config(db: Session) -> SelectionConfigSnapshot:
    """返回当前 active 的选股配置快照;无配置时抛出 RuntimeError。"""
    global _config_cache, _config_at
    now = _now()
    with _config_lock:
        if _config_cache is not None and now - _config_at < _CACHE_TTL_SECONDS:
            return _config_cache
    row = get_active_selection_config_row(db)
    snapshot = _snapshot(row)
    with _config_lock:
        _config_cache = snapshot
        _config_at = _now()
    return snapshot


def validate_selection_config(
    db: Session,
    payload: dict[str, Any],
) -> list[str]:
    """校验选股配置 payload,返回错误列表(空表示通过)。"""
    errors: list[str] = []

    try:
        parsed = SelectionConfigUpdateIn.model_validate(payload)
    except Exception as exc:
        return [str(exc)]

    enabled = {d.key: d for d in load_enabled_defs(db)}
    number_keys = {
        key for key, d in enabled.items()
        if (d.value_type or "number") == "number"
    }

    for key, weight in parsed.score_weights.items():
        if key not in enabled:
            errors.append(f"score_weights 中的 {key} 不是已定义因子")
        elif key not in number_keys:
            errors.append(f"score_weights 中的 {key} 不是数值类型因子")
        if weight <= 0 or not math.isfinite(weight):
            errors.append(f"score_weights 中的 {key} 权重必须是正有限数字")

    if parsed.vol_confirm.factor not in enabled:
        errors.append(f"vol_confirm.factor {parsed.vol_confirm.factor} 不是已定义因子")
    elif parsed.vol_confirm.factor not in number_keys:
        errors.append(f"vol_confirm.factor {parsed.vol_confirm.factor} 不是数值类型因子")

    for filt in parsed.hard_filters:
        if isinstance(filt, FactorFilter):
            if filt.factor not in enabled:
                errors.append(f"hard_filter factor {filt.factor} 不是已定义因子")
            elif filt.factor not in number_keys:
                errors.append(f"hard_filter factor {filt.factor} 不是数值类型因子")

    return errors


__all__ = [
    "HardFilter",
    "SelectionConfigSnapshot",
    "SelectionConfigUpdateIn",
    "get_active_selection_config_row",
    "invalidate_selection_config_cache",
    "load_selection_config",
    "validate_selection_config",
]
