"""策略引擎:遍历股票,对每个单标的策略计算最新信号并落库 quant_signal。

信号定义:目标仓位相对前一交易日发生跳变时产生
  0 -> 1: buy;1 -> 0: sell;不变: 无信号。
watch 信号:仓位未跳变但临近触发条件时(由策略的 watch() 判断)产出 side=watch。
组合策略(kind=portfolio)不按个股出信号,不在此跑。

跑的是 `quant_strategy` 里**所有启用的**单标的策略(公共 + 用户自建),每行按
自己的 params 算。停用的策略跳过。

**循环顺序是「每只股票加载一次日线,再跑全部策略」**,不是反过来。原实现
`for 策略: for 股票: load_bars_df(...)` 让同一只股票的日线按策略数重复查库 ——
策略从 4 个固定模板变成「所有用户启用的策略」后,这个乘数会直接把夜间流水线
拖超时。
"""
from __future__ import annotations

import logging
from datetime import date, timedelta

import pandas as pd
from sqlalchemy import delete, select
from sqlalchemy.dialects.mysql import insert as mysql_insert
from sqlalchemy.orm import Session

from ..data.calendar import is_trading_day
from ..data.ingest import load_bars_df
from ..backtest.engine import opening_buy_tradable_mask
from ..models import Signal, Strategy, WatchlistItem
from ..research_plan.service import create_holding_plan, create_single_plan
from .store import enabled_strategies
from .overlays import (DEFAULT_SIMULATION_SLIPPAGE, apply_single_overlays,
                       single_entry_price_ceiling, single_entry_price_floor)
from .strategies import REGISTRY

logger = logging.getLogger(__name__)

LOOKBACK_DAYS = 400  # 信号计算加载的历史窗口(ma60/mom60 等最长 61 条)
MIN_BARS = 60  # 数据太短不足以出可靠信号


# 落库的信号(price 用信号日收盘价)
def _save_signal(db: Session, code: str, day: date, strategy_id: int,
                 side: str, price: float, reason: dict) -> None:
    stmt = mysql_insert(Signal).values(
        code=code, date=day, strategy_id=strategy_id, side=side,
        price=price, reason=reason,
    )
    stmt = stmt.on_duplicate_key_update(price=price, reason=reason)
    db.execute(stmt)


def _save_signal_and_plan(db: Session, code: str, day: date,
                          strategy: Strategy, side: str, price: float,
                          reason: dict, df) -> None:
    """信号与其研究计划在同一保存点写入，避免出现有信号却无法解释的半状态。"""
    with db.begin_nested():
        _save_signal(db, code, day, strategy.id, side, price, reason)
        signal = db.execute(select(Signal).where(
            Signal.code == code,
            Signal.date == day,
            Signal.strategy_id == strategy.id,
            Signal.side == side,
        )).scalar_one()
        create_single_plan(db, strategy, signal, df)


def _resolve_strategies(db: Session,
                        strategies: list[Strategy] | None) -> list[Strategy]:
    """待跑的单标的策略行。None 表示库里所有启用的。"""
    if strategies is None:
        strategies = enabled_strategies(db, kind="single")
    resolved = []
    for strategy in strategies:
        mod = REGISTRY.get(strategy.template)
        if mod is None:
            # 模板在库里但代码里没有(降级部署/手改数据):明确告警而不是静默跳过
            logger.warning("策略「%s」的模板 %s 不存在于当前代码,跳过",
                           strategy.name, strategy.template)
            continue
        if mod.KIND != "single":
            continue
        resolved.append(strategy)
    return resolved


def run_signals(db: Session, day: date | None = None,
                codes: list[str] | None = None,
                strategies: list[Strategy] | None = None) -> dict:
    """对股票跑全部启用的单标的策略,产出当日信号(含 watch)。

    strategies: 策略行列表,None 表示库里所有启用的单标的策略。
    返回 {策略名: {code: side}} 汇总。
    """
    day = day or date.today()
    if not is_trading_day(db, day):
        logger.info("%s 非交易日，不生成信号或研究计划", day)
        return {"date": str(day), "signals": {}, "total": 0}
    if codes is None:
        codes = [r[0] for r in db.execute(
            select(WatchlistItem.code).distinct()).all()]
    targets = _resolve_strategies(db, strategies)
    if not targets:
        logger.info("没有启用的单标的策略,跳过信号计算")
        return {"date": str(day), "signals": {}, "total": 0}

    start = day - timedelta(days=LOOKBACK_DAYS)
    # 按 id 而不是 name 归集:不同用户可以有同名策略(唯一键是 owner_id+name),
    # 用 name 做键会让两个人的同名策略互相覆盖并让 total 少算
    summary: dict[int, dict[str, str]] = {s.id: {} for s in targets}
    names = {s.id: s.name for s in targets}
    produced: dict[tuple[int, str], set[str]] = {}  # (strategy_id, code) -> 当日 side 集

    for code in codes:
        # 每只股票只查一次库,再喂给全部策略(见模块文档字符串)
        df = load_bars_df(db, code, start=start, end=day)
        if len(df) < MIN_BARS:
            continue
        if df["date"].iat[-1] != day:
            # 该日无新数据(非交易日或数据未更新),不产生信号
            continue
        price = float(df["close"].iat[-1])
        for strategy in targets:
            mod = REGISTRY[strategy.template]
            params = strategy.params or {}
            try:
                native = mod.positions(df, params)
                dates = pd.DatetimeIndex(df["date"])
                overlay_state: dict = {}
                pos, overlay_exits = apply_single_overlays(
                    df, native, params, slippage=DEFAULT_SIMULATION_SLIPPAGE,
                    entry_tradable=opening_buy_tradable_mask(
                        {code: df}, dates)[code],
                    entry_price_ceiling=single_entry_price_ceiling(
                        mod, df, params),
                    entry_price_floor=single_entry_price_floor(
                        mod, df, params),
                    state_out=overlay_state,
                )
            except Exception:  # noqa: BLE001 - 单个策略出错不影响其他策略和股票
                logger.exception("策略 %s 计算失败 %s", strategy.name, code)
                continue
            if pos.empty:
                continue
            produced.setdefault((strategy.id, code), set())  # 已重算,允许清理旧 side
            cur, prev = int(pos.iat[-1]), int(pos.iat[-2])
            if cur != prev:
                # T 日进入信号在今日开盘受阻时，模拟仓位会从“待进入”回到 0，
                # 但原生条件仍为持有。这不是退出，不能制造一条卖出提示。
                blocked_entry = (
                    cur < prev
                    and overlay_state.get("entry_blocked") is True
                    and pd.Timestamp(day) not in overlay_exits
                )
                if blocked_entry:
                    logger.info("模拟入场未成交 %s %s，等待原生条件重置", strategy.name, code)
                    continue
                side = "buy" if cur == 1 else "sell"
                reason = {
                    "params": params,
                    "prev_position": prev,
                    "cur_position": cur,
                    "close": price,
                }
                if side == "sell" and pd.Timestamp(day) in overlay_exits:
                    reasons = overlay_exits[pd.Timestamp(day)]
                    reason["primary_exit_reason"] = reasons[0]
                    reason["all_exit_reasons"] = reasons
                if overlay_state.get("simulated_entry_price") is not None:
                    reason["simulated_entry_price"] = overlay_state[
                        "simulated_entry_price"
                    ]
                if overlay_state.get("rules"):
                    reason["overlay_state_rules"] = overlay_state["rules"]
                _save_signal_and_plan(
                    db, code, day, strategy, side, price, reason, df)
                produced[(strategy.id, code)].add(side)
                summary[strategy.id][code] = side
                logger.info("信号 %s %s %s @ %.2f",
                            strategy.name, code, side, price)
            elif cur == prev == 1:
                create_holding_plan(
                    db, strategy, code=code, data_date=day,
                    df=df,
                    simulated_entry_price=overlay_state.get(
                        "simulated_entry_price"),
                    overlay_state_rules=overlay_state.get("rules") or [],
                )
            elif hasattr(mod, "watch"):
                reason = mod.watch(df, params)
                if reason:
                    reason["params"] = params
                    _save_signal_and_plan(
                        db, code, day, strategy, "watch", price, reason, df)
                    produced[(strategy.id, code)].add("watch")
                    summary[strategy.id][code] = "watch"
                    logger.info("watch %s %s @ %.2f %s",
                                strategy.name, code, price, reason)

    # 清理重算后已失效的 side:唯一键含 side,数据修正后 buy -> watch/sell/
    # 无信号时旧记录不会被 on_duplicate_key_update 覆盖,需要显式删除
    for (strategy_id, code), sides in produced.items():
        q = delete(Signal).where(
            Signal.code == code,
            Signal.date == day,
            Signal.strategy_id == strategy_id,
        )
        if sides:
            q = q.where(Signal.side.notin_(sides))
        res = db.execute(q)
        if res.rowcount:
            logger.info("清理失效信号 strategy=%d %s %s: %d 条",
                        strategy_id, code, day, res.rowcount)
    db.commit()
    # 对外用「策略名(编号)」做键:结果只进日志和管理端点,名字可读、编号消歧
    return {
        "date": str(day),
        "signals": {f"{names[sid]}#{sid}": v for sid, v in summary.items()},
        "total": sum(len(v) for v in summary.values()),
    }
