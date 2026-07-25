"""股票基础资料的集中查询与序列化。"""
from __future__ import annotations

from collections.abc import Iterable

from sqlalchemy import select
from sqlalchemy.orm import Session

from .models import Stock

DEFAULT_CODE_CHUNK = 500


class StockRepository:
    def __init__(self, db: Session, chunk_size: int = DEFAULT_CODE_CHUNK):
        if chunk_size <= 0:
            raise ValueError("chunk_size 必须大于 0")
        self.db = db
        self.chunk_size = chunk_size

    @staticmethod
    def _unique(codes: Iterable[str] | None) -> list[str]:
        return list(dict.fromkeys(codes or []))

    def _chunks(self, codes: Iterable[str] | None):
        unique = self._unique(codes)
        for start in range(0, len(unique), self.chunk_size):
            yield unique[start:start + self.chunk_size]

    def by_codes(self, codes: Iterable[str] | None) -> dict[str, Stock]:
        stocks: dict[str, Stock] = {}
        for chunk in self._chunks(codes):
            rows = self.db.execute(
                select(Stock).where(Stock.code.in_(chunk))
            ).scalars().all()
            stocks.update((row.code, row) for row in rows)
        return stocks

    def existing_codes(self, codes: Iterable[str] | None) -> set[str]:
        existing: set[str] = set()
        for chunk in self._chunks(codes):
            existing.update(row[0] for row in self.db.execute(
                select(Stock.code).where(Stock.code.in_(chunk))
            ).all())
        return existing

    def items(self, codes: Iterable[str] | None) -> list[dict]:
        ordered = list(codes or [])
        stocks = self.by_codes(ordered)
        return [
            {
                "code": code,
                "name": stocks[code].name if code in stocks else "",
                "industry": stocks[code].industry if code in stocks else "",
            }
            for code in ordered
        ]
