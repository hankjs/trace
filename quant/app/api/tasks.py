"""全局异步任务查询/取消(任务系统本体见 app/tasks.py)。"""
from __future__ import annotations

from typing import Annotated

from fastapi import APIRouter, Depends, HTTPException, Query
from sqlalchemy import select
from sqlalchemy.orm import Session

from ..auth import require_client, user_id_from_claims
from ..db import get_db
from ..models import Task
from ..tasks import cancel_task, task_payload

router = APIRouter(prefix="/api/tasks", tags=["tasks"])


def _own_task_or_404(db: Session, task_id: int, user_id: str) -> Task:
    task = db.execute(
        select(Task).where(Task.id == task_id, Task.user_id == user_id)
    ).scalar_one_or_none()
    if task is None:
        raise HTTPException(404, f"任务 {task_id} 不存在")
    return task


@router.get("")
def list_tasks(
    limit: Annotated[int, Query(ge=1, le=200)] = 50,
    db: Session = Depends(get_db),
    claims: dict = Depends(require_client),
):
    """当前用户的任务列表,新的在前。列表不带 result(体积大),详情单取。"""
    user_id = user_id_from_claims(claims)
    tasks = db.execute(
        select(Task)
        .where(Task.user_id == user_id)
        .order_by(Task.id.desc())
        .limit(limit)
    ).scalars().all()
    return {"tasks": [task_payload(t) for t in tasks]}


@router.get("/{task_id}")
def get_task(
    task_id: int,
    db: Session = Depends(get_db),
    claims: dict = Depends(require_client),
):
    task = _own_task_or_404(db, task_id, user_id_from_claims(claims))
    return task_payload(task, with_result=True)


@router.post("/{task_id}/cancel")
def cancel(
    task_id: int,
    db: Session = Depends(get_db),
    claims: dict = Depends(require_client),
):
    """取消等待中的任务;运行中的线程无法安全中断,返回 409。"""
    task = _own_task_or_404(db, task_id, user_id_from_claims(claims))
    if not cancel_task(db, task):
        raise HTTPException(409, "仅等待中的任务可以取消")
    db.refresh(task)
    return task_payload(task)
