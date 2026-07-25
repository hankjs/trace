"""日频向量化回测引擎(vectorbt 1.1.0 实现)。

规则:
- 信号(目标仓位)在 T 日收盘产生,T+1 日开盘价成交;
- 单标的满仓/空仓(0/1 仓位),多标的时资金等分;
- 组合策略消费 target_weights 矩阵,同样 T+1 开盘按目标权重调仓;
- 起点前已持有的状态不丢:单标的若回测窗口首日目标仓位为 1,以首日开盘价
  合成建仓;组合以起点前最后一天的权重在首日开盘建仓(先 shift 再截断);
- 费用:佣金 commission(默认万 2.5)双边、滑点 slippage(默认万 1,按价格比例)、
  印花税 stamp_tax(默认 0.05%,仅卖出)全部进入撮合——fees 按 (bar, column)
  广播,卖出单 = 佣金 + 印花税;净值、胜率、后续仓位规模为同一税后口径;
- 组合调仓 call_seq="auto":同一调仓日先卖后买,买单可用卖出释放的现金,
  避免"买列先于卖列执行导致买单被拒、换仓变全现金";
- 输出净值曲线与指标:总收益率、年化、最大回撤、夏普、胜率、交易次数。

vectorbt 1.1.0 注意(与 0.x 不同,勿凭记忆):
- fees 与 slippage 是两个独立参数,slippage 直接支持,不要再加进 fees;
- fees 接受按 (bar, column) 广播的数组,用于区分买/卖方向的费率;
- 多列(批量)时 pf.value() 返回 DataFrame,pf.total_return() 等返回按列 Series;
- 组合目标权重用 Portfolio.from_orders(size=权重矩阵, size_type="targetpercent",
  cash_sharing=True, group_by=True, call_seq="auto");非调仓行给 NaN 则当日不下单;
- 订单明细在 pf.orders.records_readable(列:Column/Timestamp/Size/Price/Side/Fees)。
"""
from __future__ import annotations

import itertools
import logging
import math
from datetime import date, timedelta

import numpy as np
import pandas as pd
import vectorbt as vbt
from sqlalchemy.orm import Session

from ..data.ingest import load_bars_df
from ..data.universe import membership_intervals
from ..catalog import STRATEGIES
from ..models import BacktestEquity, BacktestRun
from ..strategy.strategies import REGISTRY, strategy_kind

logger = logging.getLogger(__name__)

DEFAULT_COSTS = {
    "commission": 0.00025,  # 佣金,双边
    "stamp_tax": 0.0005,    # 印花税,仅卖出
    "slippage": 0.0001,     # 滑点,按价格比例
}

MIN_BARS = 30
MIN_STAT_BARS = 20  # 短于此的净值序列指标不可靠,返回 None
TRADING_DAYS = 242  # A股年均实际交易日(旧代码误用 252 的日历近似)
RISK_FREE_RATE = 0.0  # 年化无风险利率,默认 0(此时夏普退化为信息比率)
PORTFOLIO_WARMUP_DAYS = 200  # 组合策略计算动量/均线需要 start 之前的历史
SINGLE_WARMUP_DAYS = 200     # 单标的策略同样需要预热,否则 MA60 等指标在区间头部失真


def _signals_from_positions(pos: pd.Series) -> tuple[pd.Series, pd.Series]:
    """目标仓位(0/1)-> entries/exits 布尔序列(T 日信号 T+1 成交:右移一日)"""
    diff = pos.astype(float).diff().fillna(0.0)
    entries = (diff > 0).shift(1, fill_value=False)
    exits = (diff < 0).shift(1, fill_value=False)
    return entries, exits


def _held_before(pos: pd.Series, first_bar: pd.Timestamp) -> bool:
    """回测起点之前是否已持仓(用于合成首日建仓)。

    看的是 first_bar **之前**最后一根 bar 的目标仓位,而不是 first_bar 当天的
    仓位:后者由当日收盘价算出,若据此在当日开盘建仓,就用上了当天收盘才知道
    的信号——一笔前视成交。起点前无历史(预热段为空)时返回 False:没有证据
    表明已持仓,交给正常的 T+1 信号流处理。
    """
    prior = pos.loc[pos.index < first_bar]
    if not len(prior):
        return False
    return float(prior.iloc[-1]) == 1.0


def _signal_fee_matrix(entries: pd.DataFrame, exits: pd.DataFrame,
                       costs: dict) -> np.ndarray:
    """from_signals 的按单费率矩阵:买入 = 佣金,卖出 = 佣金 + 印花税。"""
    fees = np.full(entries.shape, costs["commission"], dtype=float)
    fees[exits.to_numpy()] += costs["stamp_tax"]
    return fees


def _validate_costs(costs: dict | None) -> dict[str, float]:
    allowed = set(DEFAULT_COSTS)
    supplied = costs or {}
    unknown = set(supplied) - allowed
    if unknown:
        raise ValueError(f"未知费用参数: {', '.join(sorted(unknown))}")
    result = {**DEFAULT_COSTS, **supplied}
    limits = {"commission": 0.05, "stamp_tax": 0.05, "slippage": 0.10}
    for key, maximum in limits.items():
        value = result[key]
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            raise ValueError(f"{key} 必须是数字")
        value = float(value)
        if not math.isfinite(value) or not 0 <= value <= maximum:
            raise ValueError(f"{key} 必须在 0 到 {maximum} 之间")
        result[key] = value
    return result


def _validate_params(strategy: str, params: dict | None) -> dict:
    supplied = params or {}
    metadata = {item["key"]: item for item in STRATEGIES[strategy]["params"]}
    unknown = set(supplied) - set(metadata)
    if unknown:
        raise ValueError(f"{strategy} 不支持参数: {', '.join(sorted(unknown))}")
    normalized = {}
    for key, value in supplied.items():
        spec = metadata[key]
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            raise ValueError(f"参数 {key} 必须是数字")
        number = float(value)
        if not math.isfinite(number):
            raise ValueError(f"参数 {key} 必须是有限数字")
        if spec["value_type"] == "integer":
            if not number.is_integer():
                raise ValueError(f"参数 {key} 必须是整数")
            normalized[key] = int(number)
        else:
            normalized[key] = number
        if not spec["minimum"] <= number <= spec["maximum"]:
            raise ValueError(
                f"参数 {key} 必须在 {spec['minimum']} 到 {spec['maximum']} 之间"
            )
    merged = {
        item["key"]: normalized.get(item["key"], item["default"])
        for item in metadata.values()
    }
    if strategy == "ma_cross" and merged["fast"] >= merged["slow"]:
        raise ValueError("参数 fast 必须小于 slow")
    if strategy == "mean_reversion" and merged["rsi_buy"] >= merged["rsi_sell"]:
        raise ValueError("参数 rsi_buy 必须小于 rsi_sell")
    if strategy == "momentum_rotation" and merged["w_mom20"] + merged["w_mom60"] <= 0:
        raise ValueError("动量权重之和必须大于 0")
    return normalized


def _equity_statistics(eq: pd.Series, initial_cash: float = 1.0
                       ) -> tuple[float, float | None, float, pd.Series]:
    """从初始资金起算收益、年化、回撤和逐日收益，包含首日成交成本。

    - 年化基数 TRADING_DAYS=242(A股年均实际交易日),非日历近似 252;
    - 回撤为净值**自身峰谷**(cummax 不再以初始资金播种),净值 [0.5,0.4]
      得 -0.20 而非 -0.60;
    - 序列短于 MIN_STAT_BARS 时年化不可靠,返回 None(3 bar 时 242/3 次方
      会把噪声放大成天文数字)。
    """
    if not len(eq):
        raise ValueError("净值序列为空,无法计算指标")
    total_return = float(eq.iat[-1] / initial_cash - 1)
    annual: float | None = None
    if len(eq) >= MIN_STAT_BARS:
        annual = float((eq.iat[-1] / initial_cash) ** (TRADING_DAYS / len(eq)) - 1)
    running_max = eq.cummax()
    max_dd = float((eq / running_max - 1).min())
    rets = eq.pct_change()
    rets.iloc[0] = eq.iat[0] / initial_cash - 1
    return total_return, annual, max_dd, rets.dropna()


def _sharpe_ratio(rets: pd.Series, risk_free: float = RISK_FREE_RATE
                  ) -> float | None:
    """年化夏普。risk_free 为年化无风险利率,按 TRADING_DAYS 折成单期超额。

    risk_free=0 时退化为对零的信息比率(旧口径)。
    """
    if len(rets) < MIN_STAT_BARS:
        return None
    std = float(rets.std())
    if not std > 0:
        return None
    excess = float(rets.mean()) - risk_free / TRADING_DAYS
    return round(excess / std * np.sqrt(TRADING_DAYS), 4)


def _metrics_from_equity(eq: pd.Series, pf: vbt.Portfolio,
                         risk_free: float = RISK_FREE_RATE) -> dict:
    """从净值序列 + Portfolio 计算指标(费用已在撮合内扣除,口径一致)"""
    total_return, annual, max_dd, rets = _equity_statistics(eq)
    win_rate = pf.trades.win_rate()
    return {
        "total_return": round(total_return, 4),
        "annual_return": None if annual is None else round(annual, 4),
        "max_drawdown": round(max_dd, 4),
        "sharpe": _sharpe_ratio(rets, risk_free),
        "win_rate": None if pd.isna(win_rate) else round(float(win_rate), 4),
        "trade_count": int(pf.orders.count()),
        "round_trips": int(pf.trades.count()),
    }


def _to_price_matrix(dfs: dict[str, pd.DataFrame], col: str,
                     idx: pd.DatetimeIndex) -> pd.DataFrame:
    return pd.DataFrame(
        {c: d.set_index("date")[col] for c, d in dfs.items() if len(d)}
    ).reindex(idx)


def _batch_single(dfs: dict[str, pd.DataFrame], positions: dict[str, pd.Series],
                  costs: dict, start: date) -> dict[str, dict]:
    """vectorbt 批量单标的回测:同一组 entries/exits 矩阵一次跑完。

    dfs/positions 为含预热段的完整序列(positions 与 dfs[code] 行位置对齐),
    start 为回测起点:信号在完整序列上计算(diff 不丢起点前的跳变),价格
    矩阵截到 [start, ...];**起点前一根 bar** 的目标仓位为 1 时以首日开盘价
    合成建仓(起点前已持有的近似)。

    注意判定用的是 start 之前那根 bar 而非窗口首日:窗口首日的仓位是用当日
    收盘价算出来的,若拿它去当日开盘成交就等于用了当天才知道的信号
    (前视偏差)。与 _portfolio_sim 的"先 shift 再截断"同一口径。

    返回 {code: {"metrics": ..., "equity": Series}}
    """
    idx = pd.DatetimeIndex(
        sorted({d for df in dfs.values() for d in df["date"] if d >= start})
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
        e = e.reindex(idx, fill_value=False)
        x = x.reindex(idx, fill_value=False)
        # 起点前已持仓 -> 首日开盘价合成建仓。判定必须看 start 之前那根 bar 的
        # 仓位:窗口首日的仓位由当日收盘价算出,拿它当日开盘成交是前视偏差。
        if len(idx) and _held_before(p, idx[0]) and not x.iloc[0]:
            e.iloc[0] = True
        entries[code] = e
        exits[code] = x

    pf = vbt.Portfolio.from_signals(
        close, entries, exits, price=open_,
        init_cash=1.0, size=1.0, size_type="percent",
        fees=_signal_fee_matrix(entries, exits, costs),
        slippage=costs["slippage"], freq="1D",
    )
    eq_all = pf.value()
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
                 costs: dict | None = None, save: bool = True,
                 dynamic_universe: bool = False,
                 user_id: int | None = None) -> dict:
    """跑回测并(默认)落库。多标的时资金等分,组合净值为各标的净值平均。

    组合策略(KIND=portfolio)走 target_weights + vectorbt 组合回测。
    """
    mod = REGISTRY.get(strategy)
    if mod is None:
        raise ValueError(f"未知策略: {strategy},可选: {list(REGISTRY)}")
    costs = _validate_costs(costs)
    params = _validate_params(strategy, params)

    if strategy_kind(strategy) == "portfolio":
        return _run_portfolio(
            db, strategy, codes, start, end, params, costs, save,
            dynamic_universe=dynamic_universe, user_id=user_id,
        )

    warmup_start = start - timedelta(days=SINGLE_WARMUP_DAYS)
    dfs: dict[str, pd.DataFrame] = {}
    positions: dict[str, pd.Series] = {}
    for code in codes:
        # 多加载 start 之前的历史做指标预热;信号在完整序列上计算,引擎内切窗口
        df = load_bars_df(db, code, start=warmup_start, end=end)
        if len(df) == 0 or int((df["date"] >= start).sum()) < MIN_BARS:
            logger.warning("回测 %s 区间内数据不足,跳过", code)
            continue
        dfs[code] = df
        positions[code] = mod.positions(df, params)
    if not dfs:
        raise ValueError("所有标的都数据不足,无法回测")

    results = _batch_single(dfs, positions, costs, start)
    per_code = {c: r["metrics"] for c, r in results.items()}
    curves = [r["equity"] for r in results.values()]

    # 每只标的各自从同一初始资金 1.0 开始，缺失起点按现金处理，再等权合成。
    aligned = pd.concat(curves, axis=1).ffill().fillna(1.0)
    combo = aligned.mean(axis=1)
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
        _save_run(db, result, start, end, combo, user_id=user_id)
    return result


def _portfolio_sim(weights_full: pd.DataFrame, pool_dfs: dict[str, pd.DataFrame],
                   bt_idx: pd.DatetimeIndex, costs: dict) -> dict:
    """组合模拟:weights_full 为含预热段的完整目标权重矩阵(行=交易日)。

    先 shift(1) 再截断到 bt_idx:首个交易日以起点前最后一天的权重开盘建仓,
    不丢起点前已持有的组合状态。
    返回 {"equity": Series, "metrics": dict, "pf": Portfolio}
    """
    close = _to_price_matrix(pool_dfs, "close", bt_idx)
    open_ = _to_price_matrix(pool_dfs, "open", bt_idx)

    # T 日目标权重 -> T+1 开盘成交;只在权重变化的行下单(其余行 NaN)
    w_exec_full = weights_full.shift(1)
    changed_full = w_exec_full.ne(w_exec_full.shift()).any(axis=1)
    w_exec = w_exec_full.reindex(bt_idx)
    changed = changed_full.reindex(bt_idx, fill_value=False)
    if len(changed):
        changed.iloc[0] = True  # 首日建仓(起点前已有的权重)
    w_orders = w_exec.where(pd.DataFrame(
        np.repeat(changed.to_numpy()[:, None], w_exec.shape[1], axis=1),
        index=w_exec.index, columns=w_exec.columns))

    commission_fees = np.full(w_exec.shape, costs["commission"], dtype=float)
    probe = vbt.Portfolio.from_orders(
        close, size=w_orders, size_type="targetpercent", price=open_,
        init_cash=1.0, fees=commission_fees,
        slippage=costs["slippage"], cash_sharing=True, group_by=True,
        call_seq="auto", freq="1D",
    )
    fees = commission_fees.copy()
    if costs["stamp_tax"]:
        for order in probe.orders.records_readable.to_dict("records"):
            if order["Side"] == "Sell":
                row = w_exec.index.get_loc(order["Timestamp"])
                col = w_exec.columns.get_loc(order["Column"])
                fees[row, col] += costs["stamp_tax"]
        pf = vbt.Portfolio.from_orders(
            close, size=w_orders, size_type="targetpercent", price=open_,
            init_cash=1.0, fees=fees, slippage=costs["slippage"],
            cash_sharing=True, group_by=True, call_seq="auto", freq="1D",
        )
    else:
        pf = probe
    eq = pf.value().dropna()
    return {"equity": eq, "metrics": _metrics_from_equity(eq, pf), "pf": pf}


def _run_portfolio(db: Session, strategy: str, codes: list[str],
                   start: date, end: date, params: dict | None,
                   costs: dict, save: bool, *, dynamic_universe: bool,
                   user_id: int | None) -> dict:
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
    eligibility = None
    if dynamic_universe:
        idx = pd.DatetimeIndex(all_dates)
        eligibility = pd.DataFrame(False, index=idx, columns=pool_dfs)
        for member in membership_intervals(db, list(pool_dfs), warmup_start, end):
            active = idx.date >= member.in_date
            if member.out_date is not None:
                active &= idx.date < member.out_date
            eligibility.loc[active, member.code] = True
    weights_full = mod.target_weights(
        all_dates, pool_dfs, params, eligibility=eligibility,
    )
    bt_idx = pd.DatetimeIndex([d for d in all_dates if start <= d <= end])
    if len(bt_idx) < 3:
        raise ValueError("回测区间交易日不足")

    sim = _portfolio_sim(weights_full, pool_dfs, bt_idx, costs)
    eq = sim["equity"]

    result: dict = {
        "strategy": strategy,
        "params": params or {},
        "codes": codes,
        "start": str(start),
        "end": str(end),
        "costs": costs,
        "metrics": sim["metrics"],
        "equity": [
            {"date": str(d.date()), "equity": round(float(v), 6)}
            for d, v in eq.items()
        ],
    }
    if save:
        _save_run(db, result, start, end, eq, user_id=user_id)
    return result


def _combo_metrics(combo: pd.Series, per_code: dict[str, dict],
                   risk_free: float = RISK_FREE_RATE) -> dict:
    total_return, annual, max_dd, rets = _equity_statistics(combo)
    return {
        "total_return": round(total_return, 4),
        "annual_return": None if annual is None else round(annual, 4),
        "max_drawdown": round(max_dd, 4),
        "sharpe": _sharpe_ratio(rets, risk_free),
        "win_rate": _weighted_win_rate(per_code),
        "trade_count": sum(m["trade_count"] for m in per_code.values()),
        "round_trips": sum(m["round_trips"] for m in per_code.values()),
        "per_code": per_code,
    }


def _weighted_win_rate(per_code: dict[str, dict]) -> float | None:
    """组合胜率:按各标的回合交易数加权,而非算术平均。

    算术平均会让只做了 1 笔交易的标的与做了 100 笔的标的等权,
    2 笔全胜的标的把整体胜率拉高到失真。
    """
    num = den = 0.0
    for m in per_code.values():
        wr, trips = m.get("win_rate"), m.get("round_trips") or 0
        if wr is None or trips <= 0:
            continue
        num += wr * trips
        den += trips
    return round(num / den, 4) if den > 0 else None


def _save_run(db: Session, result: dict, start: date, end: date,
              combo: pd.Series, user_id: int | None = None) -> None:
    run = BacktestRun(strategy=result["strategy"], params=result["params"],
                      codes=result["codes"], start=start, end=end,
                      metrics=result["metrics"], user_id=user_id)
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
    costs = _validate_costs(costs)
    if not param_grid:
        raise ValueError("param_grid 不能为空")

    keys = list(param_grid)
    for key, values in param_grid.items():
        if not isinstance(values, list) or not values:
            raise ValueError(f"参数 {key} 的候选值必须是非空数组")
    count = math.prod(len(param_grid[key]) for key in keys)
    if count > 200:
        raise ValueError(f"参数组合过多({count}),上限 200")
    combos = []
    for vals in itertools.product(*(param_grid[k] for k in keys)):
        combos.append(_validate_params(strategy, dict(zip(keys, vals))))

    warmup_start = start - timedelta(days=SINGLE_WARMUP_DAYS)
    full_dfs: dict[str, pd.DataFrame] = {}  # 含预热段,信号在完整序列上计算
    for code in codes:
        df = load_bars_df(db, code, start=warmup_start, end=end)
        if len(df) and int((df["date"] >= start).sum()) >= MIN_BARS:
            full_dfs[code] = df
    if not full_dfs:
        raise ValueError("所有标的都数据不足,无法回测")

    rows = []
    for combo in combos:
        positions = {c: mod.positions(df, combo) for c, df in full_dfs.items()}
        results = _batch_single(full_dfs, positions, costs, start)
        per = [r["metrics"] for r in results.values()]
        rows.append({
            "params": combo,
            "metrics": {
                # 短序列的 annual_return 为 None(见 MIN_STAT_BARS),聚合要跳过
                "annual_return_mean": _mean_or_none(
                    [m["annual_return"] for m in per]),
                "annual_return_median": _median_or_none(
                    [m["annual_return"] for m in per]),
                "total_return_mean": _mean_or_none(
                    [m["total_return"] for m in per]),
                "max_drawdown_median": _median_or_none(
                    [m["max_drawdown"] for m in per]),
                "sharpe_median": _median_or_none([m["sharpe"] for m in per]),
                "win_rate_mean": _mean_or_none([m["win_rate"] for m in per]),
                "trade_count": sum(m["trade_count"] for m in per),
            },
            "per_code": {c: r["metrics"] for c, r in results.items()},
        })
    # 无法年化的组(全部标的序列过短)排在最后
    rows.sort(key=lambda r: -(r["metrics"]["annual_return_median"]
                              if r["metrics"]["annual_return_median"] is not None
                              else float("inf")))
    return {"strategy": strategy, "codes": list(full_dfs), "start": str(start),
            "end": str(end), "costs": costs, "results": rows}


def _median_or_none(vals: list) -> float | None:
    vals = [v for v in vals if v is not None]
    return round(float(np.median(vals)), 4) if vals else None


def _mean_or_none(vals: list) -> float | None:
    vals = [v for v in vals if v is not None]
    return round(float(np.mean(vals)), 4) if vals else None
