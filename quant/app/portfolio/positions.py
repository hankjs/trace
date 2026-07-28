"""持仓计算:由 quant_trade 实时推导(均价法成本),不落表。

浮动盈亏取价优先级:当日最新盘中快照 > 最近收盘价。
"""
from __future__ import annotations

import logging

from sqlalchemy import select
from sqlalchemy.orm import Session

from ..data.latest_prices import latest_reference_prices
from ..models import Trade

logger = logging.getLogger(__name__)


def _latest_prices(db: Session, codes: list[str]) -> dict[str, tuple[float, str]]:
    """每只股票的最新参考价:(价格, 来源)。批量查询。"""
    return latest_reference_prices(db, codes)


def _compute_book(db: Session, user_id: int) -> dict[str, dict]:
    """按 code 聚合所有成交,均价法推导 {code: {qty, cost, realized}}。

    买入:总成本 += price*qty + fee
    卖出:已实现盈亏 += (price - 均价)*qty - 手续费(按实际卖出数量分摊);
    数量与成本按比例扣减。已清仓的 code 也保留,供汇总已实现盈亏用。
    """
    trades = db.execute(
        select(Trade).where(Trade.user_id == user_id)
        .order_by(Trade.trade_date, Trade.id)
    ).scalars().all()

    book: dict[str, dict] = {}
    for t in trades:
        p = book.setdefault(t.code, {"qty": 0.0, "cost": 0.0, "realized": 0.0})
        if t.side == "buy":
            p["cost"] += t.price * t.qty + t.fee
            p["qty"] += t.qty
        else:  # sell
            avg = p["cost"] / p["qty"] if p["qty"] > 0 else 0.0
            sell_qty = min(t.qty, p["qty"])
            if sell_qty < t.qty:
                # 写入层(trades.add_trade)已拒绝超卖,走到这里说明库里存着
                # 早于该校验的历史脏数据。仍按持仓截断以免负持仓,但要显式
                # 标记,不能只留一行日志让差异静默消失。
                p["oversold_qty"] = p.get("oversold_qty", 0.0) + (t.qty - sell_qty)
                logger.error(
                    "卖出数量超过持仓 %s @ %s: 委托 %.0f,持仓 %.0f,"
                    "按持仓截断(疑似写入校验前的历史脏数据)",
                    t.code, t.trade_date, t.qty, p["qty"])
            # 手续费按实际卖出数量分摊,截断部分不计费
            fee_alloc = t.fee * sell_qty / t.qty if t.qty > 0 else 0.0
            p["realized"] += (t.price - avg) * sell_qty - fee_alloc
            p["qty"] -= sell_qty
            p["cost"] -= avg * sell_qty
    return book


def _position_item(
    code: str, p: dict, prices: dict[str, tuple[float, str]],
) -> dict:
    avg_cost = p["cost"] / p["qty"]
    last_price, price_src = prices.get(code, (None, None))
    item = {
        "code": code,
        "qty": p["qty"],
        "avg_cost": round(avg_cost, 4),
        "last_price": last_price,
        "price_source": price_src,
        "market_value": round(p["qty"] * last_price, 2) if last_price else None,
        "unrealized_pnl": (
            round((last_price - avg_cost) * p["qty"], 2) if last_price else None
        ),
        "realized_pnl": round(p["realized"], 2),
    }
    if p.get("oversold_qty"):
        item["data_warning"] = (
            f"存在超卖成交 {p['oversold_qty']:g} 股,已按持仓截断"
        )
    return item


def compute_positions(db: Session, user_id: int) -> list[dict]:
    """当前持仓列表(仅仍有持仓的股票)"""
    book = _compute_book(db, user_id)
    holding_codes = [c for c, p in book.items() if p["qty"] > 1e-9]
    prices = _latest_prices(db, holding_codes)
    return [_position_item(code, book[code], prices) for code in holding_codes]


def portfolio_summary(db: Session, user_id: int) -> dict:
    book = _compute_book(db, user_id)
    holding_codes = [c for c, p in book.items() if p["qty"] > 1e-9]
    prices = _latest_prices(db, holding_codes)
    positions = [_position_item(code, book[code], prices) for code in holding_codes]
    return {
        "positions": positions,
        "total_market_value": round(
            sum(p["market_value"] or 0 for p in positions), 2),
        "total_unrealized_pnl": round(
            sum(p["unrealized_pnl"] or 0 for p in positions), 2),
        # 已实现盈亏含已清仓股票,不能只对当前持仓求和
        "total_realized_pnl": round(
            sum(p["realized"] for p in book.values()), 2),
    }
