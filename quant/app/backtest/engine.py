"""日频向量化回测引擎(vectorbt 1.1.0 实现)。

规则:
- 信号(目标仓位)在 T 日收盘产生,T+1 日开盘价成交;
- 单标的满仓/空仓(0/1 仓位),多标的时资金等分;
- 组合策略消费 target_weights 矩阵,同样 T+1 开盘按目标权重调仓;
- 费用:佣金 commission(默认万 2.5)双边,滑点 slippage(默认万 1,按价格比例)
  由 vectorbt 直接收;印花税 stamp_tax(默认 0.05%,仅卖出)按订单记录精确扣减
  (vectorbt 的 fees 双边同率,不支持单边费率);
- 输出净值曲线与指标:总收益率、年化、最大回撤、夏普、胜率、交易次数。

vectorbt 1.1.0 注意(与 0.x 不同,勿凭记忆):
- fees 与 slippage 是两个独立参数,slippage 直接支持,不要再加进 fees;
- 多列(批量)时 pf.value() 返回 DataFrame,pf.total_return() 等返回按列 Series;
- 组合目标权重用 Portfolio.from_orders(size=权重矩阵, size_type="targetpercent",
  cash_sharing=True, group_by=True);非调仓行给 NaN 则当日不下单;
- 订单明细在 pf.orders.records_readable(列:Column/Timestamp/Size/Price/Side/Fees)。
"""
from __future__ import annotations

import itertools
import logging
from datetime import date, timedelta

import numpy as np
import pandas as pd
import vectorbt as vbt
from sqlalchemy.orm import Session

from ..data.ingest import load_bars_df
from ..models import BacktestEquity, BacktestRun
from ..strategy.strategies import REGISTRY, strategy_kind

logger = logging.getLogger(__name__)

DEFAULT_COSTS = {
    "commission": 0.00025,  # 佣金,双边
    "stamp_tax": 0.0005,    # 印花税,仅卖出
    "slippage": 0.0001,     # 滑点,按价格比例
}

MIN_BARS = 30
PORTFOLIO_WARMUP_DAYS = 200  # 组合策略计算动量/均线需要 start 之前的历史
SINGLE_WARMUP_DAYS = 200     # 单标的策略同样需要预热,否则 MA60 等指标在区间头部失真


def _signals_from_positions(pos: pd.Series) -> tuple[pd.Series, pd.Series]:
    """目标仓位(0/1)-> entries/exits 布尔序列(T 日信号 T+1 成交:右移一日)"""
    diff = pos.astype(float).diff().fillna(0.0)
    entries = (diff > 0).shift(1, fill_value=False)
    exits = (diff < 0).shift(1, fill_value=False)
    return entries, exits


def _equity_after_tax(pf: vbt.Portfolio, costs: dict) -> pd.Series | pd.DataFrame:
    """从净值中精确扣除卖出印花税(按订单记录逐笔)"""
    value = pf.value()
    rr = pf.orders.records_readable
    if rr is None or len(rr) == 0:
        return value
    sells = rr[rr["Side"] == "Sell"].copy()
    if sells.empty:
        return value
    sells["notional"] = sells["Size"] * sells["Price"] * costs["stamp_tax"]
    if isinstance(value, pd.DataFrame):  # 批量:按列分别扣
        out = value.copy()
        for col in value.columns:
            # records_readable 的 Column 是列标签(股票代码),不是整数序号
            s = sells[sells["Column"] == col]
            if s.empty:
                continue
            tax = s.groupby("Timestamp")["notional"].sum()
            out[col] = out[col] - tax.reindex(out.index, fill_value=0.0).cumsum()
        return out
    tax = sells.groupby("Timestamp")["notional"].sum()
    return value - tax.reindex(value.index, fill_value=0.0).cumsum()


def _metrics_from_equity(eq: pd.Series, pf: vbt.Portfolio) -> dict:
    """从(税后)净值序列 + Portfolio 计算指标"""
    total_return = float(eq.iat[-1] / eq.iat[0] - 1)
    n_days = len(eq)
    annual = float((eq.iat[-1] / eq.iat[0]) ** (252 / max(n_days, 1)) - 1)
    max_dd = float((eq / eq.cummax() - 1).min())
    rets = eq.pct_change().dropna()
    sharpe = None
    if len(rets) > 2 and float(rets.std()) > 0:
        sharpe = round(float(rets.mean() / rets.std() * np.sqrt(252)), 4)
    win_rate = pf.trades.win_rate()
    return {
        "total_return": round(total_return, 4),
        "annual_return": round(annual, 4),
        "max_drawdown": round(max_dd, 4),
        "sharpe": sharpe,
        "win_rate": None if pd.isna(win_rate) else round(float(win_rate), 4),
        "trade_count": int(pf.orders.count()),
        "round_trips": int(pf.trades.count()),
    }


def _bt_one(df: pd.DataFrame, pos: pd.Series, costs: dict) -> dict:
    """单标的回测:pos 为目标仓位(0/1),次日开盘价成交。"""
    if len(df) < 3:
        raise ValueError("数据不足,无法回测")
    idx = pd.DatetimeIndex(df["date"])
    close = pd.Series(df["close"].to_numpy(float), index=idx)
    open_ = pd.Series(df["open"].to_numpy(float), index=idx)
    entries, exits = _signals_from_positions(pd.Series(pos.to_numpy(float), index=idx))
    pf = vbt.Portfolio.from_signals(
        close, entries, exits, price=open_,
        init_cash=1.0, size=1.0, size_type="percent",
        fees=costs["commission"], slippage=costs["slippage"], freq="1D",
    )
    eq = _equity_after_tax(pf, costs)
    return {"equity": eq, "metrics": _metrics_from_equity(eq, pf)}


def _to_price_matrix(dfs: dict[str, pd.DataFrame], col: str,
                     idx: pd.DatetimeIndex) -> pd.DataFrame:
    return pd.DataFrame(
        {c: d.set_index("date")[col] for c, d in dfs.items() if len(d)}
    ).reindex(idx)


def _batch_single(dfs: dict[str, pd.DataFrame], positions: dict[str, pd.Series],
                  costs: dict) -> dict[str, dict]:
    """vectorbt 批量单标的回测:同一组 entries/exits 矩阵一次跑完。

    返回 {code: {"metrics": ..., "equity": Series}}
    """
    idx = pd.DatetimeIndex(
        sorted({d for df in dfs.values() for d in df["date"]})
    )
    close = _to_price_matrix(dfs, "close", idx)
    open_ = _to_price_matrix(dfs, "open", idx)
    entries = pd.DataFrame(False, index=idx, columns=close.columns)
    exits = pd.DataFrame(False, index=idx, columns=close.columns)
    for code, pos in positions.items():
        if code not in close.columns:
            continue
        p = pd.Series(pos.to_numpy(float), index=pd.DatetimeIndex(dfs[code]["date"]))
        e, x = _signals_from_positions(p)
        entries[code] = e.reindex(idx, fill_value=False)
        exits[code] = x.reindex(idx, fill_value=False)

    pf = vbt.Portfolio.from_signals(
        close, entries, exits, price=open_,
        init_cash=1.0, size=1.0, size_type="percent",
        fees=costs["commission"], slippage=costs["slippage"], freq="1D",
    )
    eq_all = _equity_after_tax(pf, costs)
    win_rates = pf.trades.win_rate()
    out: dict[str, dict] = {}
    for code in close.columns:
        eq = eq_all[code].dropna()
        if len(eq) < 3:
            continue
        out[code] = {
            "equity": eq,
            "metrics": _metrics_from_equity(eq, pf[code]),
        }
    return out


def run_backtest(db: Session, strategy: str, codes: list[str],
                 start: date, end: date, params: dict | None = None,
                 costs: dict | None = None, save: bool = True) -> dict:
    """跑回测并(默认)落库。多标的时资金等分,组合净值为各标的净值平均。

    组合策略(KIND=portfolio)走 target_weights + vectorbt 组合回测。
    """
    mod = REGISTRY.get(strategy)
    if mod is None:
        raise ValueError(f"未知策略: {strategy},可选: {list(REGISTRY)}")
    costs = {**DEFAULT_COSTS, **(costs or {})}

    if strategy_kind(strategy) == "portfolio":
        return _run_portfolio(db, strategy, codes, start, end, params, costs, save)

    warmup_start = start - timedelta(days=SINGLE_WARMUP_DAYS)
    dfs: dict[str, pd.DataFrame] = {}
    positions: dict[str, pd.Series] = {}
    for code in codes:
        # 多加载 start 之前的历史做指标预热,信号算完后切回 [start, end] 回测
        df = load_bars_df(db, code, start=warmup_start, end=end)
        if len(df) < MIN_BARS:
            logger.warning("回测 %s 数据不足(%d 条),跳过", code, len(df))
            continue
        pos = mod.positions(df, params)
        mask = (df["date"] >= start).to_numpy()
        df = df[mask].reset_index(drop=True)
        if len(df) < MIN_BARS:
            logger.warning("回测 %s 区间内数据不足(%d 条),跳过", code, len(df))
            continue
        dfs[code] = df
        positions[code] = pos[mask]
    if not dfs:
        raise ValueError("所有标的都数据不足,无法回测")

    results = _batch_single(dfs, positions, costs)
    per_code = {c: r["metrics"] for c, r in results.items()}
    curves = [r["equity"] for r in results.values()]

    # 组合净值:各标的归一化后等权平均
    norm = [c / c.iloc[0] for c in curves]
    combo = pd.concat(norm, axis=1).ffill().mean(axis=1)
    metrics = _combo_metrics(combo, per_code)

    result: dict = {
        "strategy": strategy,
        "params": params or {},
        "codes": codes,
        "start": str(start),
        "end": str(end),
        "costs": costs,
        "metrics": metrics,
        "equity": [
            {"date": str(d.date()), "equity": round(float(v), 6)}
            for d, v in combo.items()
        ],
    }
    if save:
        _save_run(db, result, start, end, combo)
    return result


def _run_portfolio(db: Session, strategy: str, codes: list[str],
                   start: date, end: date, params: dict | None,
                   costs: dict, save: bool) -> dict:
    """组合策略回测:target_weights -> T+1 开盘按目标权重调仓。"""
    mod = REGISTRY[strategy]
    warmup_start = start - timedelta(days=PORTFOLIO_WARMUP_DAYS)
    pool_dfs: dict[str, pd.DataFrame] = {}
    for code in codes:
        df = load_bars_df(db, code, start=warmup_start, end=end)
        if len(df) < MIN_BARS:
            continue
        pool_dfs[code] = df
    if not pool_dfs:
        raise ValueError("所有标的都数据不足,无法回测")

    all_dates = sorted({d for df in pool_dfs.values() for d in df["date"]})
    weights_full = mod.target_weights(all_dates, pool_dfs, params)
    bt_idx = pd.DatetimeIndex([d for d in all_dates if start <= d <= end])
    if len(bt_idx) < 3:
        raise ValueError("回测区间交易日不足")

    w = weights_full.reindex(bt_idx).fillna(0.0)
    close = _to_price_matrix(pool_dfs, "close", bt_idx)
    open_ = _to_price_matrix(pool_dfs, "open", bt_idx)

    # T 日目标权重 -> T+1 开盘成交;只在权重变化的行下单(其余行 NaN)
    w_exec = w.shift(1)
    changed = w_exec.ne(w_exec.shift()).any(axis=1)
    if len(w_exec):
        changed.iloc[0] = True
    w_orders = w_exec.where(pd.DataFrame(
        np.repeat(changed.to_numpy()[:, None], w_exec.shape[1], axis=1),
        index=w_exec.index, columns=w_exec.columns))

    pf = vbt.Portfolio.from_orders(
        close, size=w_orders, size_type="targetpercent", price=open_,
        init_cash=1.0, fees=costs["commission"], slippage=costs["slippage"],
        cash_sharing=True, group_by=True, freq="1D",
    )
    eq = _equity_after_tax(pf, costs)
    eq = eq.dropna()
    metrics = _metrics_from_equity(eq, pf)

    result: dict = {
        "strategy": strategy,
        "params": params or {},
        "codes": codes,
        "start": str(start),
        "end": str(end),
        "costs": costs,
        "metrics": metrics,
        "equity": [
            {"date": str(d.date()), "equity": round(float(v), 6)}
            for d, v in eq.items()
        ],
    }
    if save:
        _save_run(db, result, start, end, eq)
    return result


def _combo_metrics(combo: pd.Series, per_code: dict[str, dict]) -> dict:
    total_return = float(combo.iat[-1] - 1)
    annual = float(combo.iat[-1] ** (252 / max(len(combo), 1)) - 1)
    max_dd = float((combo / combo.cummax() - 1).min())
    rets = combo.pct_change().dropna()
    sharpe = None
    if len(rets) > 2 and float(rets.std()) > 0:
        sharpe = round(float(rets.mean() / rets.std() * np.sqrt(252)), 4)
    win_rates = [m["win_rate"] for m in per_code.values() if m["win_rate"] is not None]
    return {
        "total_return": round(total_return, 4),
        "annual_return": round(annual, 4),
        "max_drawdown": round(max_dd, 4),
        "sharpe": sharpe,
        "win_rate": round(sum(win_rates) / len(win_rates), 4) if win_rates else None,
        "trade_count": sum(m["trade_count"] for m in per_code.values()),
        "round_trips": sum(m["round_trips"] for m in per_code.values()),
        "per_code": per_code,
    }


def _save_run(db: Session, result: dict, start: date, end: date,
              combo: pd.Series) -> None:
    run = BacktestRun(strategy=result["strategy"], params=result["params"],
                      codes=result["codes"], start=start, end=end,
                      metrics=result["metrics"])
    db.add(run)
    db.flush()
    db.execute(
        BacktestEquity.__table__.insert(),
        [{"run_id": run.id, "date": d.date(), "equity": float(v)}
         for d, v in combo.items()],
    )
    db.commit()
    result["run_id"] = run.id


def run_sweep(db: Session, strategy: str, codes: list[str],
              start: date, end: date, param_grid: dict,
              costs: dict | None = None) -> dict:
    """参数扫描:param_grid = {参数名: [候选值]},笛卡尔积逐组批量回测(不落库)。"""
    mod = REGISTRY.get(strategy)
    if mod is None:
        raise ValueError(f"未知策略: {strategy}")
    if strategy_kind(strategy) != "single":
        raise ValueError("参数扫描目前只支持单标的策略")
    costs = {**DEFAULT_COSTS, **(costs or {})}
    if not param_grid:
        raise ValueError("param_grid 不能为空")

    keys = list(param_grid)
    combos = [dict(zip(keys, vals))
              for vals in itertools.product(*(param_grid[k] for k in keys))]
    if len(combos) > 200:
        raise ValueError(f"参数组合过多({len(combos)}),上限 200")

    warmup_start = start - timedelta(days=SINGLE_WARMUP_DAYS)
    full_dfs: dict[str, pd.DataFrame] = {}  # 含预热段,用于计算信号
    dfs: dict[str, pd.DataFrame] = {}       # 切回 [start, end],用于回测
    for code in codes:
        df = load_bars_df(db, code, start=warmup_start, end=end)
        bt = df[df["date"] >= start].reset_index(drop=True)
        if len(bt) >= MIN_BARS:
            full_dfs[code] = df
            dfs[code] = bt
    if not dfs:
        raise ValueError("所有标的都数据不足,无法回测")

    rows = []
    for combo in combos:
        positions = {}
        for c, df in full_dfs.items():
            pos = mod.positions(df, combo)
            positions[c] = pos[(df["date"] >= start).to_numpy()]
        results = _batch_single(dfs, positions, costs)
        per = [r["metrics"] for r in results.values()]
        annuals = [m["annual_return"] for m in per]
        rows.append({
            "params": combo,
            "metrics": {
                "annual_return_mean": round(float(np.mean(annuals)), 4),
                "annual_return_median": round(float(np.median(annuals)), 4),
                "total_return_mean": round(
                    float(np.mean([m["total_return"] for m in per])), 4),
                "max_drawdown_median": round(
                    float(np.median([m["max_drawdown"] for m in per])), 4),
                "sharpe_median": _median_or_none([m["sharpe"] for m in per]),
                "win_rate_mean": _mean_or_none([m["win_rate"] for m in per]),
                "trade_count": sum(m["trade_count"] for m in per),
            },
            "per_code": {c: r["metrics"] for c, r in results.items()},
        })
    rows.sort(key=lambda r: -r["metrics"]["annual_return_median"])
    return {"strategy": strategy, "codes": list(dfs), "start": str(start),
            "end": str(end), "costs": costs, "results": rows}


def _median_or_none(vals: list) -> float | None:
    vals = [v for v in vals if v is not None]
    return round(float(np.median(vals)), 4) if vals else None


def _mean_or_none(vals: list) -> float | None:
    vals = [v for v in vals if v is not None]
    return round(float(np.mean(vals)), 4) if vals else None
