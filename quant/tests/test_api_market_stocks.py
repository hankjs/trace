"""全市场股票清单(all=true)的契约断言。

选股器一次拉取全量做客户端过滤与虚拟滚动,契约点:
1. all=true 且空查询时下发全部股票,忽略 limit;
2. 自选股排在最前并带 is_watch 标记;
3. 不带 all 时保持原行为(空查询只回自选股,limit 生效);
4. all=true 与 q 同现时退化为普通搜索(q 优先)。
"""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from sqlalchemy import create_engine
from sqlalchemy.orm import Session
from sqlalchemy.pool import StaticPool

from app.api.market import search_stocks
from app.db import Base
from app.models import Stock, WatchlistItem

USER_A = "11111111-1111-1111-1111-111111111111"
CLAIMS_A = {"sub": USER_A, "username": "a", "can_client": True}


def _session() -> Session:
    engine = create_engine(
        "sqlite://", connect_args={"check_same_thread": False}, poolclass=StaticPool,
    )
    Base.metadata.create_all(engine)
    return Session(engine)


def _seed(db: Session) -> None:
    db.add_all([
        Stock(code="sh.600519", name="贵州茅台", industry="白酒", list_date=None, is_st=False),
        Stock(code="sz.000001", name="平安银行", industry="银行", list_date=None, is_st=False),
        Stock(code="sh.600036", name="招商银行", industry="银行", list_date=None, is_st=False),
    ])
    db.add(WatchlistItem(user_id=USER_A, code="sz.000001"))
    db.commit()


def test_all_returns_full_list_watch_first():
    db = _session()
    _seed(db)
    payload = search_stocks(q="", limit=20, all_stocks=True, db=db, claims=CLAIMS_A)
    assert payload["count"] == 3
    codes = [item["code"] for item in payload["items"]]
    # 自选(sz.000001)在最前,其余按代码排序
    assert codes == ["sz.000001", "sh.600036", "sh.600519"]
    assert payload["items"][0]["is_watch"] is True
    assert payload["items"][1]["is_watch"] is False


def test_all_ignores_limit():
    db = _session()
    _seed(db)
    payload = search_stocks(q="", limit=1, all_stocks=True, db=db, claims=CLAIMS_A)
    assert payload["count"] == 3


def test_default_empty_query_keeps_watchlist_only():
    db = _session()
    _seed(db)
    payload = search_stocks(q="", limit=20, all_stocks=False, db=db, claims=CLAIMS_A)
    assert [item["code"] for item in payload["items"]] == ["sz.000001"]


def test_all_with_query_falls_back_to_search():
    db = _session()
    _seed(db)
    payload = search_stocks(q="银行", limit=20, all_stocks=True, db=db, claims=CLAIMS_A)
    codes = {item["code"] for item in payload["items"]}
    assert codes == {"sz.000001", "sh.600036"}
