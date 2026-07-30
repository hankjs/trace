"""手工记账写入层校验:超卖必须被拒绝,不得静默截断。

对应 brief §3.7。持有 100 股却录入卖出 200 股,旧实现在 positions.py 用
min(卖出量, 持仓量) 静默截断——库里保留 200 股的成交,盈亏只算 100 股,
两边永久不一致。现在写入事务内重放整条时间序列,拒绝导致负持仓的成交。
"""
from __future__ import annotations

import sys
from datetime import date
from pathlib import Path

import pytest
from sqlalchemy import create_engine, text
from sqlalchemy.orm import Session

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.db import Base
from app.models import Trade
from app.portfolio import positions as pos_svc
from app.portfolio import trades as trade_svc

USER = 7
CODE = "sh.600519"


@pytest.fixture()
def db():
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    # 生产是 MySQL BIGINT AUTO_INCREMENT;SQLite 只对 INTEGER PRIMARY KEY
    # 自动发号,故在测试库里把 quant_trade 重建为 INTEGER 主键。
    with engine.begin() as conn:
        conn.execute(text("DROP TABLE quant_trade"))
        conn.execute(text("""
            CREATE TABLE quant_trade (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id BIGINT,
                code VARCHAR(16) NOT NULL,
                trade_date DATE NOT NULL,
                side VARCHAR(8) NOT NULL,
                price FLOAT NOT NULL,
                qty FLOAT NOT NULL,
                fee FLOAT,
                note TEXT
            )
        """))
    with Session(engine) as session:
        yield session


def _buy(db, qty: float, day: date = date(2024, 1, 2), price: float = 10.0,
         user_id: int = USER):
    return trade_svc.add_trade(db, user_id, CODE, day, "buy", price, qty)


def _sell(db, qty: float, day: date = date(2024, 1, 3), price: float = 11.0,
          user_id: int = USER):
    return trade_svc.add_trade(db, user_id, CODE, day, "sell", price, qty)


def test_oversell_is_rejected_at_write_layer(db):
    """持仓 100 卖 200:写入被拒绝,库里不留这笔成交。"""
    _buy(db, 100)

    with pytest.raises(trade_svc.OversellError, match="负持仓"):
        _sell(db, 200)

    # 关键断言:拒绝必须是"不落库",而不是落库后再截断
    rows = trade_svc.list_trades(db, USER, CODE)
    assert len(rows) == 1
    assert rows[0].side == "buy"
    assert db.query(Trade).count() == 1
    # 持仓仍是完整的 100 股,没有被幽灵卖单改动
    pos = pos_svc.compute_positions(db, USER)
    assert len(pos) == 1
    assert pos[0]["qty"] == 100
    assert pos[0]["realized_pnl"] == 0.0


def test_sell_without_any_position_is_rejected(db):
    """零持仓卖出:旧实现 avg=0/sell_qty=0 让整笔凭空消失,现在必须拒绝。"""
    with pytest.raises(trade_svc.OversellError):
        _sell(db, 100)

    assert trade_svc.list_trades(db, USER, CODE) == []
    assert pos_svc.compute_positions(db, USER) == []


def test_sell_exactly_all_shares_is_allowed(db):
    """卖出等于持仓:必须放行(边界不能误杀),清仓后已实现盈亏保留。"""
    _buy(db, 100, price=10.0)
    _sell(db, 100, price=11.0)

    assert len(trade_svc.list_trades(db, USER, CODE)) == 2
    assert pos_svc.compute_positions(db, USER) == []  # 已清仓
    summary = pos_svc.portfolio_summary(db, USER)
    assert summary["total_realized_pnl"] == pytest.approx(100.0)


def test_partial_sells_cannot_exceed_holding_in_aggregate(db):
    """多笔部分卖出累计超过持仓:最后那笔被拒绝。"""
    _buy(db, 100)
    _sell(db, 60, day=date(2024, 1, 3))
    _sell(db, 40, day=date(2024, 1, 4))  # 累计正好 100,放行

    with pytest.raises(trade_svc.OversellError):
        _sell(db, 1, day=date(2024, 1, 5))

    assert len(trade_svc.list_trades(db, USER, CODE)) == 3


def test_backdated_sell_before_the_buy_is_rejected(db):
    """补录一笔早于买入的卖出:该时点尚无持仓,必须拒绝。

    只看"当前持仓够不够"会漏掉这种情况(当前持仓 100 >= 卖出 100),
    所以校验必须重放整条时间序列。
    """
    _buy(db, 100, day=date(2024, 3, 1))

    with pytest.raises(trade_svc.OversellError):
        _sell(db, 100, day=date(2024, 2, 1))  # 买入之前

    assert len(trade_svc.list_trades(db, USER, CODE)) == 1


def test_deleting_a_buy_that_supports_later_sell_is_rejected(db):
    """删掉支撑后续卖出的买入:会造成负持仓,拒绝删除。"""
    buy = _buy(db, 100)
    _sell(db, 100)

    with pytest.raises(trade_svc.OversellError):
        trade_svc.delete_trade(db, USER, buy.id)

    assert len(trade_svc.list_trades(db, USER, CODE)) == 2


def test_oversell_check_is_scoped_per_user(db):
    """校验按用户隔离:别人的买入不能给我的卖出背书。"""
    trade_svc.add_trade(db, 999, CODE, date(2024, 1, 2), "buy", 10.0, 500)

    with pytest.raises(trade_svc.OversellError):
        _sell(db, 100)  # USER 自己没有持仓

    assert trade_svc.list_trades(db, USER, CODE) == []


def test_mysql_path_locks_existing_rows_before_replay(db, monkeypatch):
    """MySQL 分支应在重放校验前对已有成交行加 FOR UPDATE。"""
    _buy(db, 100)
    calls: list[tuple[int, str]] = []

    def fake_lock(session, user_id: int, code: str) -> None:
        calls.append((user_id, code))

    monkeypatch.setattr(trade_svc, "_lock_trades_for_update", fake_lock)
    _sell(db, 50)
    assert calls == [(USER, CODE)]

    buy2 = _buy(db, 100, day=date(2024, 1, 4))
    calls.clear()
    monkeypatch.setattr(trade_svc, "_lock_trades_for_update", fake_lock)
    trade_svc.delete_trade(db, USER, buy2.id)
    assert calls == [(USER, CODE)]
