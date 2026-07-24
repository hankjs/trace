"""日频向量化回测引擎。

规则:
- 信号(目标仓位)在 T 日收盘产生,T+1 日开盘价成交;
- 单标的满仓/空仓(0/1 仓位),多标的时资金等分;
- 费用:佣金 commission(默认万 2.5)双边收取,卖出印花税 stamp_tax(默认 0.05%),
  另有滑点 slippage(按价格比例,默认万 1);
- 输出净值曲线与指标:总收益率、年化、最大回撤、胜率、交易次数。
"""
from __future__ import annotations

import logging
from datetime import date

import numpy as np
import pandas as pd
from sqlalchemy.orm import Session

from ..data.ingest import load_bars_df
from ..models import BacktestEquity, BacktestRun
from ..strategy.strategies import REGISTRY

logger = logging.getLogger(__name__)

DEFAULT_COSTS = {
    "commission": 0.00025,  # 佣金,双边
    "stamp_tax": 0.0005,    # 印花税,仅卖出
    "slippage": 0.0001,     # 滑点,按价格比例
}


def _backtest_one(df: pd.DataFrame, pos: pd.Series, costs: dict) -> dict:
    """单标的回测。pos 为目标仓位(0/1),次日开盘价成交。"""
    if len(df) < 3:
        raise ValueError("数据不足,无法回测")

    open_p = df["open"].to_numpy(dtype=float)
    close_p = df["close"].to_numpy(dtype=float)
    target = pos.to_numpy(dtype=float)

    # T 日信号 -> T+1 开盘成交:把仓位平移一天,且按开盘价计成本
    hold = np.zeros(len(df))
    trades = []  # (成交日索引, side, price)
    cash_unit = 1.0   # 以 1 元初始资金为单位,净值 = cash + shares*price
    shares = 0.0
    buy_cost_total = 0.0  # 当前持仓的买入总花费(含费用),用于胜率
    cur_hold = 0.0

    for i in range(1, len(df)):
        desired = target[i - 1]  # 昨日收盘信号,今日开盘执行
        if desired != cur_hold:
            px = open_p[i]
            if not np.isfinite(px) or px <= 0:
                px = close_p[i]
            if desired > cur_hold:  # 买入
                slip = px * costs["slippage"]
                fee_rate = costs["commission"]
                spend = cash_unit
                exec_px = px + slip
                sh = spend * (1 - fee_rate) / exec_px
                shares += sh
                cash_unit -= spend
                buy_cost_total = spend
                trades.append({"i": i, "side": "buy", "price": exec_px})
            else:  # 卖出
                slip = px * costs["slippage"]
                exec_px = px - slip
                proceeds = shares * exec_px * (1 - costs["commission"] - costs["stamp_tax"])
                cash_unit += proceeds
                shares = 0.0
                trades.append({"i": i, "side": "sell", "price": exec_px,
                               "pnl": proceeds - buy_cost_total})
            cur_hold = desired
        hold[i] = cur_hold

    equity = cash_unit + shares * close_p
    eq = pd.Series(equity, index=df.index)

    # 指标
    total_return = float(eq.iat[-1] / eq.iat[0] - 1)
    n_days = len(eq)
    annual = float((eq.iat[-1] / eq.iat[0]) ** (252 / max(n_days, 1)) - 1)
    cummax = eq.cummax()
    drawdown = eq / cummax - 1
    max_dd = float(drawdown.min())

    round_trips = [t for t in trades if t["side"] == "sell"]
    wins = sum(1 for t in round_trips if t.get("pnl", 0) > 0)
    win_rate = float(wins / len(round_trips)) if round_trips else None

    return {
        "equity": eq,
        "metrics": {
            "total_return": round(total_return, 4),
            "annual_return": round(annual, 4),
            "max_drawdown": round(max_dd, 4),
            "win_rate": None if win_rate is None else round(win_rate, 4),
            "trade_count": len(trades),
            "round_trips": len(round_trips),
        },
    }


def run_backtest(db: Session, strategy: str, codes: list[str],
                 start: date, end: date, params: dict | None = None,
                 costs: dict | None = None, save: bool = True) -> dict:
    """跑回测并(默认)落库。多标的时资金等分,组合净值为各标的净值平均。"""
    mod = REGISTRY.get(strategy)
    if mod is None:
        raise ValueError(f"未知策略: {strategy},可选: {list(REGISTRY)}")
    costs = {**DEFAULT_COSTS, **(costs or {})}

    per_code: dict[str, dict] = {}
    curves = []
    for code in codes:
        df = load_bars_df(db, code, start=start, end=end)
        if len(df) < 30:
            logger.warning("回测 %s 数据不足(%d 条),跳过", code, len(df))
            continue
        pos = mod.positions(df, params)
        res = _backtest_one(df, pos, costs)
        curve = pd.DataFrame({"date": df["date"], "equity": res["equity"]})
        per_code[code] = {"metrics": res["metrics"], "curve": curve}
        curves.append(curve.set_index("date")["equity"])

    if not curves:
        raise ValueError("所有标的都数据不足,无法回测")

    # 组合净值:各标的归一化后等权平均
    norm = [c / c.iloc[0] for c in curves]
    combo = pd.concat(norm, axis=1).ffill().mean(axis=1)
    total_return = float(combo.iat[-1] - 1)
    annual = float(combo.iat[-1] ** (252 / max(len(combo), 1)) - 1)
    max_dd = float((combo / combo.cummax() - 1).min())
    all_metrics = [v["metrics"] for v in per_code.values()]
    round_trips = sum(m["round_trips"] for m in all_metrics)
    win_rates = [m["win_rate"] for m in all_metrics if m["win_rate"] is not None]

    metrics = {
        "total_return": round(total_return, 4),
        "annual_return": round(annual, 4),
        "max_drawdown": round(max_dd, 4),
        "win_rate": round(sum(win_rates) / len(win_rates), 4) if win_rates else None,
        "trade_count": sum(m["trade_count"] for m in all_metrics),
        "round_trips": round_trips,
        "per_code": {c: v["metrics"] for c, v in per_code.items()},
    }

    result: dict = {
        "strategy": strategy,
        "params": params or {},
        "codes": codes,
        "start": str(start),
        "end": str(end),
        "costs": costs,
        "metrics": metrics,
        "equity": [
            {"date": str(d), "equity": round(float(v), 6)}
            for d, v in combo.items()
        ],
    }

    if save:
        run = BacktestRun(strategy=strategy, params=params or {}, codes=codes,
                          start=start, end=end, metrics=metrics)
        db.add(run)
        db.flush()
        db.execute(
            BacktestEquity.__table__.insert(),
            [{"run_id": run.id, "date": d, "equity": float(v)}
             for d, v in combo.items()],
        )
        db.commit()
        result["run_id"] = run.id
    return result
