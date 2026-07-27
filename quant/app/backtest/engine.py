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
- 开盘涨停、停牌或缺 bar 的买入丢弃且不顺延；开盘跌停、停牌或缺 bar 的
  减仓/卖出保留为待执行退出，在后续首个可成交开盘继续完成；
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
from collections import Counter
from copy import deepcopy
from datetime import date, timedelta

import numpy as np
import pandas as pd
import vectorbt as vbt
from sqlalchemy.orm import Session

from ..data.ingest import load_bars_df
from ..data.universe import (INDEX_NAMES, membership_intervals,
                             pool_eligibility_matrix)
from ..catalog import STRATEGY_TEMPLATES
from ..models import BacktestEquity, BacktestRun, Pool
from ..strategy.overlays import (
    apply_portfolio_overlays,
    apply_single_overlays,
    portfolio_base_exit_reasons,
    reason as exit_reason,
    single_entry_price_ceiling,
    single_entry_price_floor,
    sort_exit_reasons,
    validate_overlay,
)
from ..strategy.strategies import resolve_module

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


def _validate_params(template: str, params: dict | None) -> dict:
    """按模板元数据校验并归一参数,返回**实际生效的全量参数**(已合并默认值)。

    策略实例的 params 与用户在回测页临时填的参数走同一条校验:库里的行同样
    可能因模板参数改名而残留无效键,不能因为「存过库」就跳过校验。
    """
    supplied = params or {}
    if template not in STRATEGY_TEMPLATES:
        raise ValueError(f"未知策略模板: {template}")
    metadata = {item["key"]: item for item in STRATEGY_TEMPLATES[template]["params"]}
    unknown = set(supplied) - set(metadata)
    if unknown:
        raise ValueError(f"{template} 不支持参数: {', '.join(sorted(unknown))}")
    normalized = {}
    for key, value in supplied.items():
        spec = metadata[key]
        if spec["value_type"] == "overlay":
            normalized[key] = validate_overlay(key, value)
            continue
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
        item["key"]: deepcopy(normalized.get(item["key"], item["default"]))
        for item in metadata.values()
    }
    if template == "ma_cross" and merged["fast"] >= merged["slow"]:
        raise ValueError("参数 fast 必须小于 slow")
    if template == "mean_reversion" and merged["rsi_buy"] >= merged["rsi_sell"]:
        raise ValueError("参数 rsi_buy 必须小于 rsi_sell")
    if template == "momentum_rotation" and merged["w_mom20"] + merged["w_mom60"] <= 0:
        raise ValueError("动量权重之和必须大于 0")
    # 返回合并后的全量参数:策略行只存用户覆盖的键,但回测要按实际生效值执行
    # 和落库(否则模板默认值一改,历史结果就无法复现)
    return merged


# 公开别名:策略 CRUD 也要按同一套规则校验参数,不该跨包导私有名。
# 引擎内部沿用 `_validate_params`(旧测试按该名断言错误消息)。
validate_params = _validate_params
validate_strategy_params = _validate_params


def _strategy_version_for_evidence(strategy, effective_params: dict) -> str | None:
    """仅当前策略实例的精确生效参数可声明其研究计划版本。"""
    current = validate_strategy_params(strategy.template, strategy.params)
    if effective_params != current:
        return None
    # 延迟导入避免研究计划参数快照反向导入本模块时形成初始化环。
    from ..research_plan.domain import parameter_snapshot, strategy_version

    snapshot = parameter_snapshot(strategy)
    return strategy_version(strategy, snapshot)


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
    return round(float(excess / std * np.sqrt(TRADING_DAYS)), 4)


def _metrics_from_equity(eq: pd.Series, pf: vbt.Portfolio,
                         risk_free: float = RISK_FREE_RATE,
                         trade_details: list[dict] | None = None) -> dict:
    """从净值序列 + Portfolio 计算指标(费用已在撮合内扣除,口径一致)"""
    total_return, annual, max_dd, rets = _equity_statistics(eq)
    if trade_details is None:
        win_rate = pf.trades.win_rate()
        trade_count = int(pf.orders.count())
        round_trips = int(pf.trades.count())
    else:
        trade_count = len(trade_details)
        round_trips = sum(item.get("closed_trades", 0) for item in trade_details)
        wins = sum(item.get("winning_trades", 0) for item in trade_details)
        win_rate = wins / round_trips if round_trips else math.nan
    return {
        "total_return": round(total_return, 4),
        "annual_return": None if annual is None else round(annual, 4),
        "max_drawdown": round(max_dd, 4),
        "sharpe": _sharpe_ratio(rets, risk_free),
        "win_rate": None if pd.isna(win_rate) else round(float(win_rate), 4),
        "trade_count": trade_count,
        "round_trips": round_trips,
    }


def _prior_trading_day(df: pd.DataFrame, execution_day: pd.Timestamp) -> pd.Timestamp | None:
    dates = pd.DatetimeIndex(df["date"])
    prior = dates[dates < execution_day]
    return prior[-1] if len(prior) else None


def _trade_details(
    pf: vbt.Portfolio,
    dfs: dict[str, pd.DataFrame],
    reason_map: dict[tuple[pd.Timestamp, str], list[dict]],
    *,
    default_exit_code: str = "native",
    exit_events: dict[tuple[pd.Timestamp, str], dict] | None = None,
) -> list[dict]:
    """将真实撮合订单转换为可审计明细，并挂接收盘信号日与全部退出原因。"""
    closed = pf.trades.records_readable
    if len(closed):
        closed = closed[closed["Status"] == "Closed"]
    grouped: dict[tuple[str, pd.Timestamp], list[dict]] = {}
    for item in closed.to_dict("records"):
        key = (str(item["Column"]), pd.Timestamp(item["Exit Timestamp"]))
        grouped.setdefault(key, []).append(item)

    details: list[dict] = []
    for order in pf.orders.records_readable.to_dict("records"):
        code = str(order["Column"])
        execution_day = pd.Timestamp(order["Timestamp"])
        side = str(order["Side"]).lower()
        if side == "sell":
            event = (exit_events or {}).get((execution_day, code))
            signal_day = event["signal_date"] if event else _prior_trading_day(
                dfs[code], execution_day,
            )
            reasons = sort_exit_reasons(
                event["reasons"] if event else reason_map.get(
                    (signal_day, code), [exit_reason(default_exit_code)],
                )
            )
        else:
            signal_day = _prior_trading_day(dfs[code], execution_day)
            reasons = [{"code": "native_entry", "name": "策略原生入场"}]
        matches = grouped.get((code, execution_day), []) if side == "sell" else []
        details.append({
            "code": code,
            "signal_date": str(signal_day.date()) if signal_day is not None else None,
            "execution_date": str(execution_day.date()),
            "execution_price": round(float(order["Price"]), 6),
            "size": round(float(order["Size"]), 10),
            "fees": round(float(order["Fees"]), 10),
            "side": side,
            "primary_reason": reasons[0] if reasons else None,
            "all_reasons": reasons,
            "tradable": True,
            "execution_status": "filled",
            "closed_trades": len(matches),
            "winning_trades": sum(float(item["PnL"]) > 0 for item in matches),
            "realized_pnl": round(sum(float(item["PnL"]) for item in matches), 10),
        })
    return details


def _exit_reason_distribution(details: list[dict]) -> dict[str, dict[str, int]]:
    primary: Counter[str] = Counter()
    all_hits: Counter[str] = Counter()
    for item in details:
        if item["side"] != "sell":
            continue
        if item.get("primary_reason"):
            primary[item["primary_reason"]["code"]] += 1
        all_hits.update(reason["code"] for reason in item.get("all_reasons", []))
    return {"by_primary": dict(primary), "all_hits": dict(all_hits)}


def _to_price_matrix(dfs: dict[str, pd.DataFrame], col: str,
                     idx: pd.DatetimeIndex) -> pd.DataFrame:
    return pd.DataFrame(
        {c: d.set_index("date")[col] for c, d in dfs.items() if len(d)}
    ).reindex(idx)


def limit_pct(code: str, is_st: bool = False) -> float:
    """按当日 ST 状态和板块返回涨跌停幅度。"""
    if is_st:
        return 0.05
    if code.lower().startswith("bj."):
        return 0.30
    bare = code.split(".")[-1]
    if bare.startswith("30") or bare.startswith("688"):
        return 0.20
    return 0.10


# 保留旧内部名和测试入口；计划适配器应优先导入公开的 limit_pct。
_limit_pct = limit_pct


def _daily_limit_pct(code: str, frame: pd.DataFrame) -> pd.Series:
    values = pd.Series(limit_pct(code), index=frame.index, dtype=float)
    if "is_st" in frame:
        is_st = frame["is_st"].fillna(False).eq(True)
        values.loc[is_st] = limit_pct(code, is_st=True)
    return values


def _limit_up_mask(dfs: dict[str, pd.DataFrame], idx: pd.DatetimeIndex
                   ) -> pd.DataFrame:
    """标记"当日开盘即涨停、买不进"的 (bar, code)。

    open 与前收都取前复权价:同一复权序列内比值即真实涨幅,除权造成的跳变
    对两者同向作用。留 0.5% 容差吸收四舍五入与复权残差,避免误判。

    突破/动量策略常在大涨次日入场,而那天开盘往往一字板买不到——不拦就是
    方向固定向上的系统性虚增。
    """
    mask = pd.DataFrame(False, index=idx, columns=list(dfs))
    for code, df in dfs.items():
        if code not in mask.columns or not len(df):
            continue
        d = df.set_index("date")
        prev_close = d["close"].shift(1)
        ratio = d["open"] / prev_close.where(prev_close > 0)
        hit = (ratio >= 1 + _daily_limit_pct(code, d) * 0.995).fillna(False)
        mask[code] = hit.reindex(idx, fill_value=False).fillna(False).astype(bool)
    return mask


def opening_buy_tradable_mask(
    dfs: dict[str, pd.DataFrame],
    idx: pd.DatetimeIndex,
) -> pd.DataFrame:
    """开盘买入可成交掩码：有量、有有效价格且未开盘涨停。"""
    open_prices = _to_price_matrix(dfs, "open", idx)
    volume = _to_price_matrix(dfs, "volume", idx)
    return (
        ~_limit_up_mask(dfs, idx).reindex(
            index=idx, columns=open_prices.columns, fill_value=False)
        & open_prices.notna()
        & open_prices.gt(0)
        & volume.gt(0).fillna(False)
    )


def _sell_tradable_mask(dfs: dict[str, pd.DataFrame], idx: pd.DatetimeIndex
                        ) -> pd.DataFrame:
    """开盘卖出可成交掩码：有有效 bar、未停牌且未开盘跌停。"""
    mask = pd.DataFrame(False, index=idx, columns=list(dfs))
    for code, df in dfs.items():
        if code not in mask.columns or not len(df):
            continue
        d = df.set_index("date")
        open_price = d["open"]
        prev_close = d["close"].shift(1)
        valid = open_price.notna() & open_price.gt(0) & prev_close.notna() & prev_close.gt(0)
        if "volume" in d:
            valid &= d["volume"].notna() & d["volume"].gt(0)
        ratio = open_price / prev_close.where(prev_close > 0)
        limit_down = ratio <= 1 - _daily_limit_pct(code, d) * 0.995
        tradable = (valid & ~limit_down.fillna(False)).astype(bool)
        mask[code] = tradable.reindex(idx, fill_value=False).fillna(False).astype(bool)
    return mask


def _single_execution_schedule(
    dfs: dict[str, pd.DataFrame],
    idx: pd.DatetimeIndex,
    entries: pd.DataFrame,
    exits: pd.DataFrame,
    reason_map: dict[tuple[pd.Timestamp, str], list[dict]],
    synthetic_entries: pd.DataFrame,
) -> tuple[pd.DataFrame, pd.DataFrame, dict[tuple[pd.Timestamp, str], dict]]:
    """按真实可成交性生成单标的订单；卖出不可成交时逐日重试。"""
    open_prices = _to_price_matrix(dfs, "open", idx)
    buy_tradable = opening_buy_tradable_mask(dfs, idx).reindex(
        columns=entries.columns, fill_value=False)
    sell_tradable = _sell_tradable_mask(dfs, idx).reindex(
        columns=entries.columns, fill_value=False,
    )
    actual_entries = pd.DataFrame(False, index=idx, columns=entries.columns)
    actual_exits = pd.DataFrame(False, index=idx, columns=entries.columns)
    events: dict[tuple[pd.Timestamp, str], dict] = {}

    for code in entries.columns:
        holding = False
        pending: dict | None = None
        synthetic_pending = False
        for day in idx:
            if holding:
                if pending is None and bool(exits.at[day, code]):
                    signal_day = _prior_trading_day(dfs[code], day)
                    pending = {
                        "signal_date": signal_day,
                        "reasons": reason_map.get(
                            (signal_day, code), [exit_reason("native")],
                        ),
                    }
                if pending is not None and bool(sell_tradable.at[day, code]):
                    actual_exits.at[day, code] = True
                    events[(day, code)] = pending
                    pending = None
                    holding = False
                continue
            if bool(synthetic_entries.at[day, code]):
                synthetic_pending = True
            if synthetic_pending:
                open_price = open_prices.at[day, code]
                if pd.notna(open_price) and float(open_price) > 0:
                    actual_entries.at[day, code] = True
                    holding = True
                    synthetic_pending = False
                continue
            if bool(entries.at[day, code]) and bool(buy_tradable.at[day, code]):
                actual_entries.at[day, code] = True
                holding = True
    return actual_entries, actual_exits, events


def _batch_single(dfs: dict[str, pd.DataFrame], positions: dict[str, pd.Series],
                  costs: dict, start: date,
                  exit_reasons_by_code: dict[
                      str, dict[pd.Timestamp, list[dict]]
                  ] | None = None) -> dict[str, dict]:
    """vectorbt 批量单标的回测:同一组 entries/exits 矩阵一次跑完。

    dfs/positions 为含预热段的完整序列(positions 与 dfs[code] 行位置对齐),
    start 为回测起点:信号在完整序列上计算(diff 不丢起点前的跳变),价格
    矩阵截到 [start, ...];**起点前一根 bar** 的目标仓位为 1 时以首日开盘价
    合成建仓(起点前已持有的近似)。

    注意判定用的是 start 之前那根 bar 而非窗口首日:窗口首日的仓位是用当日
    收盘价算出来的,若拿它去当日开盘成交就等于用了当天才知道的信号
    (前视偏差)。与 _portfolio_sim 的"先 shift 再截断"同一口径。

    入场当日开盘一字涨停的,该笔**丢弃不顺延**(见 decisions D6)。

    返回 {code: {"metrics": ..., "equity": Series}}
    """
    idx = pd.DatetimeIndex(
        sorted({d for df in dfs.values() for d in df["date"] if d >= start})
    )
    close = _to_price_matrix(dfs, "close", idx)
    open_ = _to_price_matrix(dfs, "open", idx)
    entries = pd.DataFrame(False, index=idx, columns=close.columns)
    exits = pd.DataFrame(False, index=idx, columns=close.columns)
    synthetic_entries = pd.DataFrame(False, index=idx, columns=close.columns)
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
            synthetic_entries.at[idx[0], code] = True
        entries[code] = e
        exits[code] = x

    reason_map = {
        (pd.Timestamp(day), code): reasons
        for code, by_day in (exit_reasons_by_code or {}).items()
        for day, reasons in by_day.items()
    }
    entries, exits, exit_events = _single_execution_schedule(
        dfs, idx, entries, exits, reason_map, synthetic_entries,
    )

    pf = vbt.Portfolio.from_signals(
        close, entries, exits, price=open_,
        init_cash=1.0, size=1.0, size_type="percent",
        fees=_signal_fee_matrix(entries, exits, costs),
        slippage=costs["slippage"], freq="1D",
    )
    eq_all = pf.value()
    details = _trade_details(pf, dfs, reason_map, exit_events=exit_events)
    out: dict[str, dict] = {}
    for code in close.columns:
        eq = eq_all[code].dropna()
        if len(eq) < 3:
            continue
        code_details = [item for item in details if item["code"] == code]
        out[code] = {
            "equity": eq,
            "metrics": _metrics_from_equity(eq, pf[code], trade_details=code_details),
            "trade_details": code_details,
            "exit_reason_distribution": _exit_reason_distribution(code_details),
        }
    return out


def run_backtest(db: Session, strategy, codes: list[str],
                 start: date, end: date, params: dict | None = None,
                 costs: dict | None = None, save: bool = True,
                 dynamic_universe: bool = False,
                 user_id: str | None = None,
                 pool_id: int | None = None) -> dict:
    """跑回测并(默认)落库。多标的时资金等分,组合净值为各标的净值平均。

    `strategy` 是 `quant_strategy` 的行(模板 + 参数)。`params` 为可选的临时
    覆盖(回测页调参),覆盖优先于策略行自身的 params —— 这让「试参数」不必先
    存策略,存策略也不妨碍临时试别的值。

    组合模板(KIND=portfolio)走 target_weights + vectorbt 组合回测。
    """
    mod = resolve_module(strategy)
    costs = _validate_costs(costs)
    # 策略行的 params 打底,调用方的临时覆盖在上
    params = _validate_params(
        strategy.template, {**(strategy.params or {}), **(params or {})})

    if mod.KIND == "portfolio":
        return _run_portfolio(
            db, strategy, codes, start, end, params, costs, save,
            dynamic_universe=dynamic_universe, user_id=user_id,
            pool_id=pool_id,
        )

    warmup_start = start - timedelta(days=SINGLE_WARMUP_DAYS)
    dfs: dict[str, pd.DataFrame] = {}
    positions: dict[str, pd.Series] = {}
    exit_reasons: dict[str, dict[pd.Timestamp, list[dict]]] = {}
    for code in codes:
        # 多加载 start 之前的历史做指标预热;信号在完整序列上计算,引擎内切窗口
        df = load_bars_df(db, code, start=warmup_start, end=end)
        if len(df) == 0 or int((df["date"] >= start).sum()) < MIN_BARS:
            logger.warning("回测 %s 区间内数据不足,跳过", code)
            continue
        dfs[code] = df
        native = mod.positions(df, params)
        dates = pd.DatetimeIndex(df["date"])
        entry_tradable = opening_buy_tradable_mask({code: df}, dates)[code]
        positions[code], exit_reasons[code] = apply_single_overlays(
            df, native, params, slippage=costs["slippage"],
            entry_tradable=entry_tradable,
            entry_price_ceiling=single_entry_price_ceiling(mod, df, params),
            entry_price_floor=single_entry_price_floor(mod, df, params),
        )
    if not dfs:
        raise ValueError("所有标的都数据不足,无法回测")

    results = _batch_single(
        dfs, positions, costs, start, exit_reasons_by_code=exit_reasons,
    )
    per_code = {c: r["metrics"] for c, r in results.items()}
    curves = [r["equity"] for r in results.values()]

    # 每只标的各自从初始资金 1.0 开始,再等权合成。
    # 起点尚无数据的标的用 NaN 而非 1.0:填 1.0 等于把它当"净值恰好不变的
    # 现金"计入分母,会把等权平均系统性拉向零收益。mean(skipna) 只对当时
    # 真实存在的标的取平均,标的陆续上市时分母随之变化。
    aligned = pd.concat(curves, axis=1).ffill()
    combo = aligned.mean(axis=1)
    metrics = _combo_metrics(combo, per_code)
    trade_details = sorted(
        (item for result in results.values() for item in result["trade_details"]),
        key=lambda item: (item["execution_date"], item["code"], item["side"]),
    )
    reason_distribution = _exit_reason_distribution(trade_details)
    fee_assumptions = _fee_assumptions(costs)
    evidence = {
        "strategy_version": _strategy_version_for_evidence(strategy, params),
        "parameter_snapshot": deepcopy(params),
        "fee_assumptions": fee_assumptions,
        "trade_details": trade_details,
        "exit_reason_distribution": reason_distribution,
        "start": str(start),
        "end": str(end),
    }
    # BacktestRun 现有 schema 将 metrics 持久化；把证据嵌入其中，历史查询不会
    # 因策略参数或默认费率变化而丢失复现依据。
    metrics["evidence"] = evidence

    result: dict = {
        "strategy_id": strategy.id,
        "strategy_name": strategy.name,
        "template": strategy.template,
        # params 存**实际生效**的全量参数(_validate_params 已合并默认值),
        # 不是用户显式填写的子集:默认值一改历史结果就无法复现
        "params": params,
        "parameter_snapshot": deepcopy(params),
        # codes 存剔除数据不足标的后的实际样本;requested_codes 留存请求列表
        "codes": list(results),
        "requested_codes": codes,
        "start": str(start),
        "end": str(end),
        "costs": costs,
        "fee_assumptions": fee_assumptions,
        "metrics": metrics,
        "evidence": evidence,
        "trade_details": trade_details,
        "exit_reason_distribution": reason_distribution,
        "equity": [
            {"date": str(d.date()), "equity": round(float(v), 6)}
            for d, v in combo.items()
        ],
    }
    if save:
        _save_run(db, result, start, end, combo, user_id=user_id,
                  pool_id=pool_id)
    return result


def _fee_assumptions(costs: dict[str, float]) -> dict:
    return {
        "commission": {
            "rate": costs["commission"], "applies_to": "buy_and_sell",
            "name": "佣金（双边）",
        },
        "stamp_tax": {
            "rate": costs["stamp_tax"], "applies_to": "sell_only",
            "name": "印花税（仅卖出）",
        },
        "slippage": {
            "rate": costs["slippage"], "applies_to": "execution_price",
            "name": "滑点（按模拟成交价比例）",
        },
        "timing": "T 日收盘确认，T+1 日开盘模拟成交",
        "currency": "CNY",
    }


def _portfolio_execution_schedule(
    weights_full: pd.DataFrame,
    pool_dfs: dict[str, pd.DataFrame],
    bt_idx: pd.DatetimeIndex,
    w_exec: pd.DataFrame,
    planned_orders: pd.DataFrame,
    reason_map: dict[tuple[pd.Timestamp, str], list[dict]],
    synthetic_entries: pd.DataFrame,
) -> tuple[pd.DataFrame, dict[tuple[pd.Timestamp, str], dict]]:
    """组合订单可成交调度：买入失败丢弃，卖出失败保留并逐日重试。"""
    open_prices = _to_price_matrix(pool_dfs, "open", bt_idx)
    valid_open = open_prices.notna() & open_prices.gt(0)
    volume = _to_price_matrix(pool_dfs, "volume", bt_idx)
    buy_tradable = opening_buy_tradable_mask(pool_dfs, bt_idx).reindex(
        columns=w_exec.columns, fill_value=False)
    sell_tradable = _sell_tradable_mask(pool_dfs, bt_idx).reindex(
        columns=w_exec.columns, fill_value=False,
    )
    previous_targets = w_exec.shift().fillna(0.0)
    actual = pd.DataFrame(np.nan, index=bt_idx, columns=w_exec.columns)
    events: dict[tuple[pd.Timestamp, str], dict] = {}

    for code in w_exec.columns:
        pending: dict | None = None
        synthetic_pending_target: float | None = None
        blocked_buy_until_reset = False
        for day in bt_idx:
            target = planned_orders.at[day, code]
            if pending is not None:
                if bool(sell_tradable.at[day, code]):
                    actual.at[day, code] = pending["target"]
                    events[(day, code)] = pending
                    pending = None
                # 有未完成退出时忽略后续目标，先完成原始退出。
                continue
            if bool(synthetic_entries.at[day, code]):
                synthetic_pending_target = float(target)
            if synthetic_pending_target is not None:
                if bool(valid_open.at[day, code]):
                    actual.at[day, code] = synthetic_pending_target
                    synthetic_pending_target = None
                continue
            intended = w_exec.at[day, code]
            if blocked_buy_until_reset:
                if pd.notna(intended) and float(intended) <= 1e-12:
                    blocked_buy_until_reset = False
                    if not pd.isna(target):
                        actual.at[day, code] = 0.0
                else:
                    # 买入失败不因其他股票调仓而在后续被间接补单。
                    continue
            if pd.isna(target):
                continue
            previous = float(previous_targets.at[day, code])
            target = float(target)
            if target < previous - 1e-12:
                prior = weights_full.index[weights_full.index < day]
                signal_day = prior[-1] if len(prior) else None
                event = {
                    "signal_date": signal_day,
                    "reasons": reason_map.get(
                        (signal_day, code), [exit_reason("rebalance")],
                    ),
                    "target": target,
                }
                if bool(sell_tradable.at[day, code]):
                    actual.at[day, code] = target
                    events[(day, code)] = event
                else:
                    pending = event
            elif target > previous + 1e-12:
                if bool(buy_tradable.at[day, code]):
                    actual.at[day, code] = target
                else:
                    blocked_buy_until_reset = True
            else:
                actual.at[day, code] = target
    return actual, events


def _enforce_actual_portfolio_order_tradability(
    w_orders: pd.DataFrame,
    close: pd.DataFrame,
    open_: pd.DataFrame,
    pool_dfs: dict[str, pd.DataFrame],
    bt_idx: pd.DatetimeIndex,
    costs: dict,
    synthetic_entries: pd.DataFrame,
    exit_events: dict[tuple[pd.Timestamp, str], dict],
    reason_map: dict[tuple[pd.Timestamp, str], list[dict]],
    weights_full: pd.DataFrame,
) -> tuple[pd.DataFrame, dict[tuple[pd.Timestamp, str], dict]]:
    """按 vectorbt 实际订单方向二次约束整行目标权重调仓。

    组合任一成分变化时会提交整行目标权重；价格漂移可能让“目标权重未变”的
    股票也产生买卖单，所以只比较前后目标权重不足以判断涨跌停约束。
    """
    orders = w_orders.copy()
    events = dict(exit_events)
    buy_tradable = opening_buy_tradable_mask(pool_dfs, bt_idx).reindex(
        columns=orders.columns, fill_value=False,
    )
    sell_tradable = _sell_tradable_mask(pool_dfs, bt_idx).reindex(
        columns=orders.columns, fill_value=False,
    )
    forced_sells: set[tuple[pd.Timestamp, str]] = set()
    max_updates = max(1, orders.size * 3)

    for _ in range(max_updates):
        probe = vbt.Portfolio.from_orders(
            close, size=orders, size_type="targetpercent", price=open_,
            init_cash=1.0, fees=costs["commission"],
            slippage=costs["slippage"], cash_sharing=True, group_by=True,
            call_seq="auto", freq="1D",
        )
        changed = False
        for record in probe.orders.records_readable.to_dict("records"):
            day = pd.Timestamp(record["Timestamp"])
            code = str(record["Column"])
            side = str(record["Side"]).lower()
            key = (day, code)
            if bool(synthetic_entries.at[day, code]):
                continue
            if key in forced_sells and side != "sell":
                orders.at[day, code] = math.nan
                events.pop(key, None)
                forced_sells.discard(key)
                changed = True
                break
            tradable = (
                bool(buy_tradable.at[day, code])
                if side == "buy" else bool(sell_tradable.at[day, code])
            )
            if tradable:
                continue

            target = orders.at[day, code]
            orders.at[day, code] = math.nan
            if side == "sell" and pd.notna(target):
                future = bt_idx[(bt_idx > day) & sell_tradable[code].to_numpy()]
                if len(future):
                    retry_day = pd.Timestamp(future[0])
                    # 待执行减仓优先于这期间的新目标，保留首次信号日和原因。
                    orders.loc[(orders.index > day) & (orders.index < retry_day), code] = math.nan
                    orders.at[retry_day, code] = float(target)
                    event = events.pop(key, None)
                    if event is None:
                        prior = weights_full.index[weights_full.index < day]
                        signal_day = prior[-1] if len(prior) else None
                        event = {
                            "signal_date": signal_day,
                            "reasons": reason_map.get(
                                (signal_day, code), [exit_reason("rebalance")],
                            ),
                            "target": float(target),
                        }
                    events[(retry_day, code)] = event
                    forced_sells.add((retry_day, code))
            changed = True
            break
        if not changed:
            actual_keys = {
                (pd.Timestamp(row["Timestamp"]), str(row["Column"]))
                for row in probe.orders.records_readable.to_dict("records")
                if str(row["Side"]).lower() == "sell"
            }
            return orders, {
                key: value for key, value in events.items() if key in actual_keys
            }
    raise RuntimeError("组合订单可成交性约束未能收敛")


def _portfolio_sim(weights_full: pd.DataFrame, pool_dfs: dict[str, pd.DataFrame],
                   bt_idx: pd.DatetimeIndex, costs: dict,
                   exit_reasons: dict[
                       tuple[pd.Timestamp, str], list[dict]
                   ] | None = None) -> dict:
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
    planned_orders = w_exec.where(pd.DataFrame(
        np.repeat(changed.to_numpy()[:, None], w_exec.shape[1], axis=1),
        index=w_exec.index, columns=w_exec.columns))
    synthetic_entries = pd.DataFrame(False, index=bt_idx, columns=w_exec.columns)
    if len(bt_idx) and (weights_full.index < bt_idx[0]).any():
        synthetic_entries.loc[bt_idx[0]] = w_exec.loc[bt_idx[0]].fillna(0.0).gt(0)
    reason_map = exit_reasons or {}
    w_orders, exit_events = _portfolio_execution_schedule(
        weights_full, pool_dfs, bt_idx, w_exec, planned_orders, reason_map,
        synthetic_entries,
    )
    w_orders, exit_events = _enforce_actual_portfolio_order_tradability(
        w_orders, close, open_, pool_dfs, bt_idx, costs, synthetic_entries,
        exit_events, reason_map, weights_full,
    )

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
    details = _trade_details(
        pf, pool_dfs, reason_map, default_exit_code="rebalance",
        exit_events=exit_events,
    )
    return {
        "equity": eq,
        "metrics": _metrics_from_equity(eq, pf, trade_details=details),
        "pf": pf,
        "trade_details": details,
        "exit_reason_distribution": _exit_reason_distribution(details),
    }


def _run_portfolio(db: Session, strategy, codes: list[str],
                   start: date, end: date, params: dict | None,
                   costs: dict, save: bool, *, dynamic_universe: bool,
                   user_id: str | None, pool_id: int | None = None) -> dict:
    """组合策略回测:target_weights -> T+1 开盘按目标权重调仓。

    params 已由 run_backtest 校验合并,这里不再重复校验。
    """
    mod = resolve_module(strategy)
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
        pool = db.get(Pool, pool_id) if pool_id is not None else None
        if pool is None:
            # 周度批量评估沿用跨指数并集，但仍必须逐日还原成分。
            eligibility = pd.DataFrame(False, index=idx, columns=pool_dfs)
            for member in membership_intervals(
                db, list(pool_dfs), warmup_start, end,
            ):
                active = idx.date >= member.in_date
                if member.out_date is not None:
                    active &= idx.date < member.out_date
                eligibility.loc[active, member.code] = True
        else:
            eligibility = pool_eligibility_matrix(
                db, idx, list(pool_dfs), kind=pool.kind,
                index_name=pool.ref if pool.ref in INDEX_NAMES else None,
                min_list_days=pool.min_list_days, daily_frames=pool_dfs,
            )
    weights_full = mod.target_weights(
        all_dates, pool_dfs, params, eligibility=eligibility,
    )
    base_exit_reasons = portfolio_base_exit_reasons(
        strategy.template, weights_full, pool_dfs,
        mod.rebalance_mask(all_dates),
    )
    weights_full, overlay_exit_reasons, _ = apply_portfolio_overlays(
        weights_full,
        pool_dfs,
        params,
        mod.rebalance_mask(all_dates),
        slippage=costs["slippage"],
        entry_tradable=opening_buy_tradable_mask(
            pool_dfs, pd.DatetimeIndex(all_dates),
        ),
        base_exit_reasons=base_exit_reasons,
    )
    bt_idx = pd.DatetimeIndex([d for d in all_dates if start <= d <= end])
    if len(bt_idx) < 3:
        raise ValueError("回测区间交易日不足")

    sim = _portfolio_sim(
        weights_full, pool_dfs, bt_idx, costs,
        exit_reasons=overlay_exit_reasons,
    )
    eq = sim["equity"]
    fee_assumptions = _fee_assumptions(costs)
    evidence = {
        "strategy_version": _strategy_version_for_evidence(strategy, params),
        "parameter_snapshot": deepcopy(params),
        "fee_assumptions": fee_assumptions,
        "trade_details": sim["trade_details"],
        "exit_reason_distribution": sim["exit_reason_distribution"],
        "start": str(start),
        "end": str(end),
    }
    sim["metrics"]["evidence"] = evidence

    result: dict = {
        "strategy_id": strategy.id,
        "strategy_name": strategy.name,
        "template": strategy.template,
        # 同 run_backtest:params 为实际生效的全量参数,codes 为实际样本
        "params": params,
        "parameter_snapshot": deepcopy(params),
        "codes": list(pool_dfs),
        "requested_codes": codes,
        "start": str(start),
        "end": str(end),
        "costs": costs,
        "fee_assumptions": fee_assumptions,
        "metrics": sim["metrics"],
        "evidence": evidence,
        "trade_details": sim["trade_details"],
        "exit_reason_distribution": sim["exit_reason_distribution"],
        "equity": [
            {"date": str(d.date()), "equity": round(float(v), 6)}
            for d, v in eq.items()
        ],
    }
    if save:
        _save_run(db, result, start, end, eq, user_id=user_id, pool_id=pool_id)
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
              combo: pd.Series, user_id: str | None = None,
              pool_id: int | None = None) -> None:
    """落库一次回测。costs 快照与实际样本一并存,否则费率/默认参数一改,
    历史结果就无法审计复现。pool_id 存所用股票池,供按编号回查时回显。

    params 存的是**实际生效的全量参数**,不是策略行的 params —— 策略行可以被
    用户改,历史回测不能随之变化。
    """
    run = BacktestRun(
        strategy_id=result["strategy_id"], params=result["params"],
        codes=result["codes"], start=start, end=end,
        metrics=result["metrics"], user_id=user_id,
        costs=result["costs"], pool_id=pool_id,
    )
    db.add(run)
    db.flush()
    db.execute(
        BacktestEquity.__table__.insert(),
        [{"run_id": run.id, "date": d.date(), "equity": float(v)}
         for d, v in combo.items()],
    )
    db.commit()
    result["run_id"] = run.id


def run_sweep(db: Session, strategy, codes: list[str],
              start: date, end: date, param_grid: dict,
              costs: dict | None = None) -> dict:
    """参数扫描:param_grid = {参数名: [候选值]},笛卡尔积逐组批量回测(不落库)。

    扫描的是**模板参数**,策略行只用来定模板 —— 每组候选值都是完整的一组参数,
    策略行自身的 params 不参与合并(否则扫描结果取决于当前存的值,不可复现)。
    """
    mod = resolve_module(strategy)
    if mod.KIND != "single":
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
        candidate: dict = {}
        for key, value in zip(keys, vals):
            if "." not in key:
                candidate[key] = value
                continue
            parent, child = key.split(".", 1)
            if parent not in {"risk_overlay", "take_profit"}:
                raise ValueError(f"不支持嵌套扫描参数: {key}")
            candidate.setdefault(parent, {})[child] = value
        combos.append(_validate_params(strategy.template, candidate))

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
        positions = {}
        exit_reasons = {}
        for code, df in full_dfs.items():
            native = mod.positions(df, combo)
            dates = pd.DatetimeIndex(df["date"])
            positions[code], exit_reasons[code] = apply_single_overlays(
                df, native, combo, slippage=costs["slippage"],
                entry_tradable=opening_buy_tradable_mask(
                    {code: df}, dates)[code],
                entry_price_ceiling=single_entry_price_ceiling(
                    mod, df, combo),
                entry_price_floor=single_entry_price_floor(
                    mod, df, combo),
            )
        results = _batch_single(
            full_dfs, positions, costs, start,
            exit_reasons_by_code=exit_reasons,
        )
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
    return {"strategy_id": strategy.id, "strategy_name": strategy.name,
            "template": strategy.template,
            "codes": list(full_dfs), "start": str(start),
            "end": str(end), "costs": costs, "results": rows}


def _median_or_none(vals: list) -> float | None:
    vals = [v for v in vals if v is not None]
    return round(float(np.median(vals)), 4) if vals else None


def _mean_or_none(vals: list) -> float | None:
    vals = [v for v in vals if v is not None]
    return round(float(np.mean(vals)), 4) if vals else None
