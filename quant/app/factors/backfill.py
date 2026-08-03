"""因子回填任务处理器。

在 app/api/factors.py 导入本模块时注册 handler,与 backtest 任务采用
同一套全局异步任务系统。
"""
from __future__ import annotations

import logging
from datetime import date, timedelta
from typing import Any

from sqlalchemy import delete, select
from sqlalchemy.orm import Session

from ..data.calendar import is_trading_day
from ..data.ingest import SNAPSHOT_SPEC_FIELDS, load_bars_df_bulk
from ..data.universe import current_pool, pool_at
from ..models import FactorDaily, FactorDef, Task, TradeCalendar
from ..selection.pipeline import LOOKBACK_DAYS
from ..strategy.spec import expression_mode, _walk_expression, parse_expression
from ..tasks import register_handler
from .defs import can_write_factor, invalidate_factor_cache, load_all_defs, load_enabled_defs

logger = logging.getLogger(__name__)


def _used_fields(expr: dict) -> set[str]:
    return {
        node.name
        for node in _walk_expression(parse_expression(expr))
        if node.op == "field" and node.name is not None
    }


def _snapshot_fields_for_defs(defs: list[FactorDef]) -> list[str]:
    needed: set[str] = set()
    for d in defs:
        needed |= _used_fields(d.expression)
    return sorted(needed & set(SNAPSHOT_SPEC_FIELDS))


def _trading_days_in_range(db: Session, start: date, end: date) -> list[date]:
    """返回 [start, end] 内的交易日;若日历表无数据则逐日降级判断。"""
    rows = db.execute(
        select(TradeCalendar.date).where(
            TradeCalendar.date >= start,
            TradeCalendar.date <= end,
            TradeCalendar.is_open.is_(True),
        ).order_by(TradeCalendar.date)
    ).scalars().all()
    if rows:
        return list(rows)
    # 日历缺失时降级:遍历自然日并调用工作日判断
    days: list[date] = []
    current = start
    while current <= end:
        if is_trading_day(db, current):
            days.append(current)
        current += timedelta(days=1)
    return days


def _resolve_codes(db: Session, day: date, explicit_codes: list[str] | None) -> list[str]:
    if explicit_codes:
        return list(dict.fromkeys(explicit_codes))
    codes = pool_at(db, day)
    if not codes and day >= date.today():
        codes = current_pool(db)
    return codes


def _evaluate_day(
    db: Session,
    day: date,
    defs: list[FactorDef],
    codes: list[str],
) -> dict[tuple[str, date], dict[str, float]]:
    """计算某一天、给定 codes 的因子值,返回 {(code,date): values}。"""
    min_len = max((d.min_bars for d in defs), default=1)
    window_days = max(LOOKBACK_DAYS, min_len * 2)
    start = day - timedelta(days=window_days)
    extra_fields = _snapshot_fields_for_defs(defs)

    bars_by_code = load_bars_df_bulk(
        db, codes, start=start, end=day, extra_fields=extra_fields,
    )
    from .engine import evaluate_def_last

    result: dict[tuple[str, date], dict[str, float]] = {}
    for code, df in bars_by_code.items():
        if len(df) < min_len or df["date"].iat[-1] != day:
            continue
        values: dict[str, float] = {}
        for d in defs:
            if len(df) >= d.min_bars:
                v = evaluate_def_last(d, df)
                if v is not None:
                    values[d.key] = v
        if values:
            result[(code, day)] = values
    return result


def _load_existing_rows(
    db: Session,
    day: date,
    codes: list[str],
) -> dict[tuple[str, date], FactorDaily]:
    if not codes:
        return {}
    rows = db.execute(
        select(FactorDaily).where(
            FactorDaily.date == day,
            FactorDaily.code.in_(codes),
        )
    ).scalars()
    return {(r.code, r.date): r for r in rows}


def run_factor_backfill_task(db: Session, task: Task) -> dict[str, Any]:
    """回填因子任务处理器。

    支持回填单个因子(factor_key 非空)或全部启用因子(factor_key 为空)。
    按交易日逐日计算,合并到已有 quant_factor_daily 行,不覆盖其它因子键。
    """
    params = task.params or {}
    start = date.fromisoformat(params["start"])
    end = date.fromisoformat(params["end"])
    factor_key = params.get("factor_key")
    explicit_codes = params.get("codes")
    owner_id = params.get("owner_id")
    is_admin = bool(params.get("is_admin"))

    if factor_key:
        def_ = db.execute(
            select(FactorDef).where(FactorDef.key == factor_key)
        ).scalar_one_or_none()
        if def_ is None:
            raise ValueError(f"因子 {factor_key} 不存在")
        # 归属守卫:非 admin 只能回填自己的非系统因子。owner_id 为 None 表示
        # 调用方未声明身份(旧调用路径),按 is_admin 处理。
        if owner_id is not None and not can_write_factor(
            def_, user_id=owner_id, is_admin=is_admin,
        ):
            raise ValueError(f"无权回填因子 {factor_key}")
        # 截面因子依赖当期全池,逐股回填算不出来
        if expression_mode(parse_expression(def_.expression)) == "cross_section":
            raise ValueError(
                "截面因子不支持回填:截面值依赖当期全池,请直接用 factor.evaluate"
            )
        defs = [def_]
    else:
        if owner_id is not None and not is_admin:
            raise ValueError("回填全部启用因子仅管理员可用")
        defs = load_enabled_defs(db)

    if not defs:
        return {"days": 0, "rows_written": 0, "factors": 0, "skipped": 0}

    trading_days = _trading_days_in_range(db, start, end)
    rows_written = 0
    skipped = 0

    for day in trading_days:
        codes = _resolve_codes(db, day, explicit_codes)
        if not codes:
            continue
        computed = _evaluate_day(db, day, defs, codes)
        if not computed:
            continue
        existing = _load_existing_rows(db, day, list({code for code, _ in computed}))

        to_insert: list[dict] = []
        for (code, day), new_values in computed.items():
            old = existing.get((code, day))
            if old is not None:
                merged = {**(old.values or {}), **new_values}
                if merged == (old.values or {}):
                    skipped += 1
                    continue
                to_insert.append({"code": code, "date": day, "values": merged})
            else:
                to_insert.append({"code": code, "date": day, "values": new_values})

        if to_insert:
            codes_to_delete = [item["code"] for item in to_insert]
            db.execute(delete(FactorDaily).where(
                FactorDaily.date == day,
                FactorDaily.code.in_(codes_to_delete),
            ))
            db.execute(FactorDaily.__table__.insert(), to_insert)
            db.commit()
            rows_written += len(to_insert)
            logger.info("因子回填 %s: 写入 %d 行", day, len(to_insert))

    invalidate_factor_cache()
    return {
        "days": len(trading_days),
        "rows_written": rows_written,
        "factors": len(defs),
        "skipped": skipped,
    }


register_handler("factor_backfill", run_factor_backfill_task)


__all__ = ["run_factor_backfill_task"]
