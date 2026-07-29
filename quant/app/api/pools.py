"""股票池组 CRUD。

池分两类:
- **预置池**(`is_system=true`,kind='index'/'all'):全用户可读、**不可改不可删**。
  没有 `quant_pool_member` 行,成分由 `universe.resolve_pool` 按当日动态解析。
- **自定义池**(kind='static'):按 `owner_id` 归属 + `quant_pool_grant` 授权,
  直查 `quant_pool_member`。
  只存代码不存日期,用于历史区间时带幸存者偏差(前端按 kind 标注)。

`GET /{id}/members` 对预置池返回**当日解析出的当前成分**而不是空列表——
前端「另存为自定义池」拿这份快照当初始成员,返回空会静默建出空池。
"""
from __future__ import annotations

import re
from datetime import date

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, Field
from sqlalchemy import delete, func, or_, select
from sqlalchemy.exc import IntegrityError
from sqlalchemy.orm import Session

from ..auth import require_client, user_id_from_claims
from ..data.latest_prices import latest_quotes
from ..data.universe import (DEFAULT_MIN_LIST_DAYS, INDEX_NAMES,
                             IncompleteListingDataError,
                             MissingIndexHistoryError, resolve_pool,
                             resolve_pool_during)
from ..db import get_db
from ..models import (BacktestRun, Pool, PoolGrant, PoolMember,
                      ResearchPlan, StrategyEval)
from ..stock_repository import StockRepository

router = APIRouter(prefix="/api/pools", tags=["pools"])

_CODE_RE = re.compile(r"^(sh|sz|bj)\.\d{6}$")
# 单次批量导入上限。必须容得下「全部A股另存为自定义池」——A股总数已超 5000,
# 卡在 2000 会让预置全A 池的另存为直接 422。留到 10000 覆盖可预见的扩容。
MAX_CODES_PER_REQUEST = 10000
# min_list_days 上下界:0 = 不过滤新股,上限 10 年(超出即等于空池,无意义)
MIN_LIST_DAYS_MAX = 3650


class PoolCreateIn(BaseModel):
    """新建自定义池。codes 可选:预置池「另存为」时一步带上初始成员。"""

    name: str = Field(..., min_length=1, max_length=64)
    min_list_days: int = Field(DEFAULT_MIN_LIST_DAYS, ge=0, le=MIN_LIST_DAYS_MAX)
    codes: list[str] = Field(default_factory=list, max_length=MAX_CODES_PER_REQUEST)


class PoolPatchIn(BaseModel):
    name: str | None = Field(None, min_length=1, max_length=64)
    min_list_days: int | None = Field(None, ge=0, le=MIN_LIST_DAYS_MAX)


class PoolMembersIn(BaseModel):
    codes: list[str] = Field(..., min_length=1, max_length=MAX_CODES_PER_REQUEST)


def is_preset(pool: Pool) -> bool:
    """预置池:系统级共享,只读。"""
    return bool(pool.is_system)


def visible_to(user_id: str):
    """可见性条件。收成一个函数,避免散落的判断漏写导致越权或漏数据。

    = 系统池 OR 我的池 OR 授权给我的池。系统池靠 is_system 短路,不在
    quant_pool_grant 插行(见 alembic 0011)。
    """
    granted = select(PoolGrant.pool_id).where(PoolGrant.user_id == user_id)
    return or_(
        Pool.is_system.is_(True),
        Pool.owner_id == user_id,
        Pool.id.in_(granted),
    )


def can_edit(db: Session, pool: Pool, user_id: str) -> bool:
    """可写:自己的池,或被授权 can_edit。系统池一律不可写。"""
    if pool.is_system:
        return False
    if pool.owner_id == user_id:
        return True
    grant = db.get(PoolGrant, (pool.id, user_id))
    return bool(grant and grant.can_edit)


def pool_out(pool: Pool, member_count: int | None = None) -> dict:
    """池的对外形状(与前端 `Pool` 类型一致)。"""
    return {
        "id": pool.id,
        "kind": pool.kind,
        "ref": pool.ref,
        "name": pool.name,
        "min_list_days": pool.min_list_days,
        # 前端据 is_system 判断只读(取代旧的「user_id 为空即预置」)
        "is_system": bool(pool.is_system),
        "owner_id": pool.owner_id,
        "member_count": member_count,
        "created_at": pool.created_at.isoformat(sep=" ") if pool.created_at else None,
    }


def pool_ref_out(pool: Pool) -> dict:
    """筛选/回测响应里回显的池信息(前端 `PoolRef`)。"""
    return {
        "id": pool.id,
        "name": pool.name,
        "kind": pool.kind,
        # 静态池无成员历史,用于历史区间即带幸存者偏差
        "has_survivorship_bias": pool.kind == "static",
    }


def get_pool_or_404(db: Session, pool_id: int, user_id: str) -> Pool:
    """按可见性取池(见 visible_to);不可见的池按不存在处理。

    刻意返回 404 而不是 403:否则可以靠状态码枚举出别人建了哪些池。
    """
    pool = db.execute(
        select(Pool).where(Pool.id == pool_id, visible_to(user_id))
    ).scalar_one_or_none()
    if pool is None:
        raise HTTPException(404, f"股票池 {pool_id} 不存在")
    return pool


def default_pool(db: Session) -> Pool | None:
    """默认池:预置的全A(kind='all');缺失时退回 id 最小的预置池。

    与前端 `pools.ts` 的 defaultPool 同口径(优先 kind='all')。
    """
    pool = db.execute(
        select(Pool).where(Pool.is_system.is_(True), Pool.kind == "all")
        .order_by(Pool.id)
    ).scalars().first()
    if pool is not None:
        return pool
    return db.execute(
        select(Pool).where(Pool.is_system.is_(True)).order_by(Pool.id)
    ).scalars().first()


def _writable_pool(db: Session, pool_id: int, user_id: str) -> Pool:
    """取池并要求可写:预置池一律拒绝。"""
    pool = get_pool_or_404(db, pool_id, user_id)
    if is_preset(pool):
        raise HTTPException(403, f"「{pool.name}」是系统预置池，不能修改，请先另存为自定义池")
    return pool


def index_name_of(pool: Pool) -> str | None:
    """kind='index' 池的 ref -> resolve_pool 的 index_name。

    'hs300_zz500' 等组合口径落到 None(跨全部指数取并集)。
    """
    return pool.ref if pool.ref in INDEX_NAMES else None


def resolve_pool_codes(db: Session, pool: Pool, day: date | None = None) -> list[str]:
    """池在 day(默认今日)的成分代码。统一走 universe.resolve_pool,不重写口径。"""
    return _resolved(lambda: resolve_pool(
        db, day or date.today(), kind=pool.kind,
        index_name=index_name_of(pool), pool_id=pool.id,
        min_list_days=pool.min_list_days,
    ))


def resolve_pool_codes_during(db: Session, pool: Pool, start: date,
                              end: date) -> list[str]:
    """池在 [start, end] 区间的成分并集(回测样本口径)。"""
    return _resolved(lambda: resolve_pool_during(
        db, start, end, kind=pool.kind, index_name=index_name_of(pool),
        pool_id=pool.id, min_list_days=pool.min_list_days,
    ))


def _resolved(call) -> list[str]:
    """把池解析的数据完整性错误翻译成 422,而不是 500。"""
    try:
        return call()
    except IncompleteListingDataError as exc:
        # 全A 口径缺 list_date 时宁可报错也不返回半个池子(见 universe.all_market_pool)
        raise HTTPException(422, str(exc)) from exc
    except MissingIndexHistoryError as exc:
        # 指数名录未覆盖查询日:静默返回空池会让用户误读为「这天没有符合
        # 条件的股票」,而真相是成分数据没回填到那么早
        raise HTTPException(422, str(exc)) from exc
    except ValueError as exc:
        raise HTTPException(422, str(exc)) from exc


def _member_counts(db: Session, pool_ids: list[int]) -> dict[int, int]:
    """静态池成员数(预置池无成员行,不在此列)。"""
    if not pool_ids:
        return {}
    rows = db.execute(
        select(PoolMember.pool_id, func.count())
        .where(PoolMember.pool_id.in_(pool_ids))
        .group_by(PoolMember.pool_id)
    ).all()
    return {pool_id: count for pool_id, count in rows}


def _split_codes(db: Session, codes: list[str]) -> tuple[list[str], list[str]]:
    """规范化并按「是否已入库」分流,返回 (可入池代码, 被忽略代码)。

    格式非法或 `quant_stock` 里查不到的代码进 skipped —— 部分成功而不是整单
    422:用户粘贴 50 个代码有 2 个没入库,应当导入 48 个并提示,而不是全丢。
    """
    normalized: list[str] = []
    skipped: list[str] = []
    seen: set[str] = set()
    for raw in codes:
        code = raw.strip().lower()
        if not code or code in seen:
            continue
        seen.add(code)
        (normalized if _CODE_RE.fullmatch(code) else skipped).append(code)
    if not normalized:
        return [], skipped
    known = StockRepository(db).existing_codes(normalized)
    accepted = [code for code in normalized if code in known]
    skipped.extend(code for code in normalized if code not in known)
    return accepted, sorted(skipped)


@router.get("")
def list_pools(db: Session = Depends(get_db),
               claims: dict = Depends(require_client)):
    """可见池:系统预置池 + 本人自定义池。预置池排在前面。"""
    user_id = user_id_from_claims(claims)
    pools = db.execute(
        select(Pool).where(
            visible_to(user_id)
        ).order_by(Pool.is_system.desc(), Pool.id)
    ).scalars().all()
    # 预置池成员按当日动态解析,列表页不逐池解析(全A 要扫全表);
    # member_count 只对静态池给出,前端按缺省处理。
    counts = _member_counts(db, [p.id for p in pools if p.kind == "static"])
    return {
        "count": len(pools),
        "items": [
            pool_out(p, counts.get(p.id, 0) if p.kind == "static" else None)
            for p in pools
        ],
    }


@router.get("/{pool_id}")
def get_pool(pool_id: int, db: Session = Depends(get_db),
             claims: dict = Depends(require_client)):
    pool = get_pool_or_404(db, pool_id, user_id_from_claims(claims))
    count = None
    if pool.kind == "static":
        count = _member_counts(db, [pool.id]).get(pool.id, 0)
    return pool_out(pool, count)


@router.post("", status_code=201)
def create_pool(body: PoolCreateIn, db: Session = Depends(get_db),
                claims: dict = Depends(require_client)):
    """新建自定义静态池。带 codes 时一并写入成员(预置池「另存为」用)。"""
    user_id = user_id_from_claims(claims)
    name = body.name.strip()
    if not name:
        raise HTTPException(400, "股票池名称不能为空")
    pool = Pool(kind="static", ref=None, owner_id=user_id, is_system=False,
                name=name, min_list_days=body.min_list_days)
    db.add(pool)
    try:
        db.flush()
    except IntegrityError as exc:
        db.rollback()
        raise HTTPException(409, f"股票池「{name}」已存在") from exc

    accepted, skipped = _split_codes(db, body.codes)
    for code in accepted:
        db.add(PoolMember(pool_id=pool.id, code=code))
    db.commit()
    db.refresh(pool)
    result = pool_out(pool, len(accepted))
    if skipped:
        result["skipped"] = skipped
    return result


@router.patch("/{pool_id}")
def update_pool(pool_id: int, body: PoolPatchIn, db: Session = Depends(get_db),
                claims: dict = Depends(require_client)):
    pool = _writable_pool(db, pool_id, user_id_from_claims(claims))
    if body.name is not None:
        name = body.name.strip()
        if not name:
            raise HTTPException(400, "股票池名称不能为空")
        pool.name = name
    if body.min_list_days is not None:
        pool.min_list_days = body.min_list_days
    try:
        db.commit()
    except IntegrityError as exc:
        db.rollback()
        raise HTTPException(409, f"股票池「{body.name}」已存在") from exc
    db.refresh(pool)
    return pool_out(pool, _member_counts(db, [pool.id]).get(pool.id, 0))


def _pool_reference_counts(db: Session, pool_id: int) -> dict[str, int]:
    """池的历史引用数(回测/评估/研究计划,均无 pool_id 外键)。

    与策略删除的引用保护同理(见 app/api/strategies.py):这些记录是「可复现
    审计资产」,池删了 pool_id 就悬空,审计快照指向不存在的池。
    """
    refs: dict[str, int] = {}
    for label, model in (("回测", BacktestRun), ("评估", StrategyEval),
                         ("研究计划", ResearchPlan)):
        count = db.execute(
            select(func.count()).select_from(model)
            .where(model.pool_id == pool_id)
        ).scalar_one()
        if count:
            refs[label] = int(count)
    return refs


@router.delete("/{pool_id}")
def delete_pool(pool_id: int, db: Session = Depends(get_db),
                claims: dict = Depends(require_client)):
    """删除自定义池及其成员。deleted 为被删除的池 id。

    仍被回测/评估/研究计划引用时返回 409,与策略删除的保护同款。
    """
    pool = _writable_pool(db, pool_id, user_id_from_claims(claims))
    refs = _pool_reference_counts(db, pool.id)
    if refs:
        detail = "、".join(f"{label} {n} 条" for label, n in refs.items())
        raise HTTPException(
            409,
            f"「{pool.name}」仍被历史记录引用({detail})，不能删除，"
            "否则这些审计记录的 pool_id 会悬空。请先删除相关记录，或保留该池",
        )
    db.execute(delete(PoolMember).where(PoolMember.pool_id == pool.id))
    db.delete(pool)
    db.commit()
    return {"deleted": pool_id}


@router.get("/{pool_id}/members")
def list_pool_members(pool_id: int, db: Session = Depends(get_db),
                      claims: dict = Depends(require_client)):
    """池成员。预置池返回**当日解析出的当前成分**(供「另存为」取快照)。

    每项附最新参考价(盘中快照优先于最近收盘),供成员表格展示。
    """
    pool = get_pool_or_404(db, pool_id, user_id_from_claims(claims))
    codes = resolve_pool_codes(db, pool)
    items = StockRepository(db).items(codes)
    quotes = latest_quotes(db, [item["code"] for item in items])
    for item in items:
        quote = quotes.get(item["code"]) or {}
        item["price"] = quote.get("price")
        item["pct_chg"] = quote.get("pct_chg")
        item["price_ts"] = quote.get("ts")
        item["price_source"] = quote.get("source")
    return {"count": len(items), "items": items, "resolved": is_preset(pool)}


@router.post("/{pool_id}/members", status_code=201)
def add_pool_members(pool_id: int, body: PoolMembersIn,
                     db: Session = Depends(get_db),
                     claims: dict = Depends(require_client)):
    """批量加成员。未入库/格式非法的代码进 skipped,其余照常写入(部分成功)。"""
    pool = _writable_pool(db, pool_id, user_id_from_claims(claims))
    accepted, skipped = _split_codes(db, body.codes)
    existing = {
        r[0] for r in db.execute(
            select(PoolMember.code).where(PoolMember.pool_id == pool.id)
        ).all()
    }
    added = [code for code in accepted if code not in existing]
    for code in added:
        db.add(PoolMember(pool_id=pool.id, code=code))
    db.commit()
    return {
        "added": len(added),
        "skipped": skipped,
        "items": StockRepository(db).items(added),
    }


@router.delete("/{pool_id}/members/{code}")
def remove_pool_member(pool_id: int, code: str, db: Session = Depends(get_db),
                       claims: dict = Depends(require_client)):
    pool = _writable_pool(db, pool_id, user_id_from_claims(claims))
    member = db.get(PoolMember, (pool.id, code.strip().lower()))
    if member is None:
        raise HTTPException(404, f"{code} 不在股票池「{pool.name}」中")
    db.delete(member)
    db.commit()
    return {"deleted": 1, "code": code.strip().lower()}
