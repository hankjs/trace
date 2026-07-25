from __future__ import annotations

import sys
from pathlib import Path

from sqlalchemy import create_engine
from sqlalchemy.orm import Session

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.db import Base
from app.models import Stock
from app.stock_repository import StockRepository


def test_stock_repository_chunks_queries_and_preserves_item_order():
    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    with Session(engine) as db:
        db.add_all([
            Stock(code=f"sh.{index:06d}", name=f"股票{index}", industry="测试")
            for index in range(5)
        ])
        db.commit()
        repository = StockRepository(db, chunk_size=2)
        requested = ["sh.000004", "sh.000000", "sh.999999", "sh.000004"]

        assert set(repository.by_codes(requested)) == {"sh.000000", "sh.000004"}
        assert repository.existing_codes(requested) == {"sh.000000", "sh.000004"}
        assert repository.items(requested) == [
            {"code": "sh.000004", "name": "股票4", "industry": "测试"},
            {"code": "sh.000000", "name": "股票0", "industry": "测试"},
            {"code": "sh.999999", "name": "", "industry": ""},
            {"code": "sh.000004", "name": "股票4", "industry": "测试"},
        ]
