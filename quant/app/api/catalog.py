"""面向前端的指标、策略、信号与回测固定目录。"""
from __future__ import annotations

from fastapi import APIRouter, Depends
from sqlalchemy.orm import Session

from ..catalog import catalog_payload
from ..db import get_db

router = APIRouter(prefix="/api/catalog", tags=["catalog"])


@router.get("")
def get_catalog(db: Session = Depends(get_db)):
    """返回中文优先的研究元数据单一来源。"""
    return catalog_payload(db)
