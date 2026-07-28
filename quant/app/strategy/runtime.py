"""StrategySpec 运行快照与可复现指纹。

所有运行入口先把数据库行解析为不可变规格快照，后续编译、数据加载和落库都只
使用该快照。这样即使异步任务执行前用户原地修改策略，也不会重新读到新规则。
"""
from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
from datetime import date
from typing import Any, Mapping

import pandas as pd

from .spec import StrategySpec, parse_strategy_spec, strategy_spec_hash


@dataclass(frozen=True)
class StrategyExecutionSnapshot:
    spec: StrategySpec
    spec_snapshot: dict[str, Any]
    spec_hash: str
    compiler_version: str
    component_versions: dict[str, str]


def strategy_spec_for(strategy: Any) -> StrategySpec:
    """解析策略当前规格；旧测试数据仅通过预置规格完成迁移期兜底。"""
    raw = getattr(strategy, "spec", None)
    if raw:
        return parse_strategy_spec(raw)

    # 生产迁移后 spec 为 NOT NULL。这个兼容分支只服务于旧 SQLite fixture 和
    # 升级过程中的既有行，返回的仍是完整 StrategySpec，执行器不会按模板分支。
    template = getattr(strategy, "template", None)
    if not template:
        raise ValueError(f"策略「{getattr(strategy, 'name', '')}」缺少 StrategySpec")
    from .presets import get_preset_spec

    return get_preset_spec(template, getattr(strategy, "params", None))


def build_execution_snapshot(
    strategy: Any,
    *,
    compiler_version: str,
    component_versions: Mapping[str, str],
    spec_override: StrategySpec | dict[str, Any] | None = None,
) -> StrategyExecutionSnapshot:
    spec = (
        parse_strategy_spec(spec_override)
        if spec_override is not None else strategy_spec_for(strategy)
    )
    snapshot = spec.model_dump(mode="json")
    return StrategyExecutionSnapshot(
        spec=spec,
        spec_snapshot=snapshot,
        spec_hash=strategy_spec_hash(spec),
        compiler_version=compiler_version,
        component_versions=dict(sorted(component_versions.items())),
    )


def canonical_json_hash(value: Any) -> str:
    payload = json.dumps(
        value,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
        allow_nan=False,
        default=str,
    )
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def data_fingerprint(frames: Mapping[str, pd.DataFrame]) -> str:
    """对实际进入编译器的逐日数据做内容哈希，而非只哈希查询参数。"""
    digest = hashlib.sha256()
    preferred = (
        "date", "open", "high", "low", "close", "raw_close", "volume",
        "amount", "is_st", "pe_ttm", "pb", "ps_ttm", "market_cap", "roe",
        "revenue_growth", "profit_growth", "gross_margin", "debt_ratio",
        "cashflow_quality",
    )
    for code in sorted(frames):
        frame = frames[code]
        columns = [column for column in preferred if column in frame.columns]
        normalized = frame.loc[:, columns].copy()
        if "date" in normalized:
            normalized["date"] = pd.to_datetime(normalized["date"]).dt.strftime("%Y-%m-%d")
            normalized = normalized.sort_values("date")
        digest.update(code.encode("utf-8"))
        digest.update(b"\0")
        digest.update(
            normalized.to_json(
                orient="split", date_format="iso", double_precision=12,
                force_ascii=True,
            ).encode("utf-8")
        )
        digest.update(b"\0")
    return digest.hexdigest()


def universe_fingerprint(
    *,
    requested_codes: list[str],
    actual_codes: list[str],
    start: date,
    end: date,
    pool_id: int | None,
    dynamic_universe: bool,
    eligibility: pd.DataFrame | None = None,
) -> str:
    value: dict[str, Any] = {
        "requested_codes": sorted(set(requested_codes)),
        "actual_codes": sorted(set(actual_codes)),
        "start": str(start),
        "end": str(end),
        "pool_id": pool_id,
        "dynamic_universe": dynamic_universe,
    }
    if eligibility is not None:
        normalized = eligibility.sort_index().sort_index(axis=1).fillna(False).astype(bool)
        value["eligibility"] = {
            "index": [str(pd.Timestamp(item).date()) for item in normalized.index],
            "columns": list(normalized.columns),
            "values": normalized.astype(int).values.tolist(),
        }
    return canonical_json_hash(value)


def cost_fingerprint(costs: Mapping[str, float]) -> str:
    return canonical_json_hash({key: float(costs[key]) for key in sorted(costs)})


def execution_fingerprint(
    *,
    spec_hash: str,
    compiler_version: str,
    component_versions: Mapping[str, str],
    data_hash: str,
    universe_hash: str,
    cost_hash: str,
) -> str:
    components = json.dumps(
        dict(sorted(component_versions.items())),
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    )
    payload = "".join((
        spec_hash,
        compiler_version,
        components,
        data_hash,
        universe_hash,
        cost_hash,
    ))
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


__all__ = [
    "StrategyExecutionSnapshot", "build_execution_snapshot", "canonical_json_hash",
    "cost_fingerprint", "data_fingerprint", "execution_fingerprint",
    "strategy_spec_for", "universe_fingerprint",
]
