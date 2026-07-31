"""factor.preview skill。"""
from __future__ import annotations

import math
from typing import Any

from ...data.ingest import load_bars_df
from ...factors import build_reason_tree, evaluate_factor
from ...strategy.spec import parse_expression, validate_expression
from ._common import A2AContext


MAX_PREVIEW_CODES = 5
MAX_PREVIEW_DAYS = 120


def _available_fields(db):
    from ...data.ingest import BAR_FIELDS, snapshot_available_fields
    return BAR_FIELDS | snapshot_available_fields(db)


def _preview_one(db, expression: dict[str, Any], code: str, days: int) -> dict[str, Any]:
    from ...api.factors import _used_fields
    from ...data.ingest import SNAPSHOT_SPEC_FIELDS

    needed = _used_fields(expression)
    extra_fields = sorted(needed & set(SNAPSHOT_SPEC_FIELDS))
    df = load_bars_df(db, code, extra_fields=extra_fields or None)
    if df.empty:
        return {
            "code": code,
            "dates": [],
            "values": [],
            "reason_tree": {},
            "error": f"未找到 {code} 的日线数据",
        }
    series = evaluate_factor(expression, df)
    full_fields = {col: df[col] for col in df.columns if col != "date"}
    reason_tree = build_reason_tree(parse_expression(expression), full_fields, position=-1)
    df = df.tail(days).reset_index(drop=True)
    series = series.tail(days).reset_index(drop=True)
    dates = [str(d) for d in df["date"]]
    values: list[float | None] = []
    for v in series:
        if v is None or (isinstance(v, float) and (math.isnan(v) or not math.isfinite(v))):
            values.append(None)
        else:
            values.append(round(float(v), 12))
    return {
        "code": code,
        "dates": dates,
        "values": values,
        "reason_tree": reason_tree,
        "error": None,
    }


def handle(payload: dict[str, Any], ctx: A2AContext, cancel_event=None) -> dict[str, Any]:
    """多标的预览；code/codes 合并去重，上限 5，days 上限 120。"""
    expression = payload.get("expression")
    if expression is None:
        raise ValueError("payload 必须包含 expression")
    result = validate_expression(
        expression, require_type="number", available_fields=_available_fields(ctx.db),
    )
    if not result.valid:
        raise ValueError("表达式校验失败")

    codes: list[str] = []
    if payload.get("code"):
        codes.append(str(payload["code"]))
    if payload.get("codes"):
        codes.extend(str(c) for c in payload["codes"])
    codes = list(dict.fromkeys(codes))
    if not codes:
        raise ValueError("必须提供 code 或 codes")

    truncated = len(codes) > MAX_PREVIEW_CODES
    codes = codes[:MAX_PREVIEW_CODES]
    days = max(1, min(int(payload.get("days") or 60), MAX_PREVIEW_DAYS))

    items = [_preview_one(ctx.db, expression, c, days) for c in codes]
    return {
        "factor_preview": {
            "items": items,
            "truncated_codes": truncated,
            "note": "spot_check_only_not_market_efficacy",
        }
    }


__all__ = ["handle"]
