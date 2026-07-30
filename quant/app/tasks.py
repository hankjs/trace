"""全局异步任务系统:提交即返回、线程池后台执行、每用户同时单任务。

耗时操作(回测/参数扫描/成本敏感性…)统一走这里:
HTTP 层 ``submit_task()`` 插入 pending 行后返回 202;线程池 worker 按 type
分发到注册的 handler 执行。每个用户同时只允许一个 pending/running 任务,
提交冲突由 HTTP 层转成 409。

handler 注册:各业务模块(如 app/api/backtest.py)在 import 时调用
``register_handler()``,避免本模块反向依赖业务层。

进程崩溃/重启的恢复见 ``recover_tasks()``:running 一律标记 failed
(执行线程已随进程消失,不可能完成),pending 重新派发。
"""
from __future__ import annotations

import logging
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime
from typing import Any, Callable

from sqlalchemy import select, update
from sqlalchemy.orm import Session

from .config import settings
from .db import SessionLocal
from .models import BacktestRun, Task

logger = logging.getLogger(__name__)

PENDING = "pending"
RUNNING = "running"
DONE = "done"
FAILED = "failed"
CANCELLED = "cancelled"
ACTIVE_STATUSES = (PENDING, RUNNING)

Handler = Callable[[Session, Task], dict[str, Any] | None]
HANDLERS: dict[str, Handler] = {}

_executor: ThreadPoolExecutor | None = None


class TaskConflictError(Exception):
    """同一用户已有进行中的任务。"""


def register_handler(task_type: str, handler: Handler) -> None:
    HANDLERS[task_type] = handler


def _get_executor() -> ThreadPoolExecutor:
    global _executor
    if _executor is None:
        _executor = ThreadPoolExecutor(
            max_workers=max(1, settings.task_workers),
            thread_name_prefix="quant-task",
        )
    return _executor


def user_active_task(db: Session, user_id: str) -> Task | None:
    return db.execute(
        select(Task)
        .where(Task.user_id == user_id, Task.status.in_(ACTIVE_STATUSES))
        .order_by(Task.id.desc())
        .limit(1)
    ).scalar_one_or_none()


def submit_task(
    db: Session,
    *,
    user_id: str,
    type: str,
    title: str,
    params: dict | None = None,
    ref_id: int | None = None,
) -> Task:
    """提交任务:冲突检查 → 插入 pending 行 → 派发执行。

    ``settings.task_async=False``(测试)时同步内联执行,返回时任务已到终态。
    """
    if user_active_task(db, user_id) is not None:
        raise TaskConflictError("已有进行中的任务,请等待完成后再提交")
    task = Task(
        user_id=user_id, type=type, status=PENDING, title=title,
        params=params or {}, ref_id=ref_id, created_at=datetime.now(),
    )
    db.add(task)
    db.commit()
    db.refresh(task)
    if settings.task_async:
        _get_executor().submit(run_task, task.id)
    else:
        run_task(task.id)
    return task


def run_task(task_id: int) -> None:
    """worker 入口:原子抢占 pending → running,分发执行,落终态。"""
    with SessionLocal() as db:
        try:
            claimed = db.execute(
                update(Task)
                .where(Task.id == task_id, Task.status == PENDING)
                .values(status=RUNNING, started_at=datetime.now(), error=None)
            )
            db.commit()
            if claimed.rowcount != 1:
                logger.info("任务 %s 不可抢占(已处理或已取消)", task_id)
                return
            task = db.get(Task, task_id)
            handler = HANDLERS.get(task.type)
            if handler is None:
                raise RuntimeError(f"未注册的任务类型: {task.type}")
            result = handler(db, task)
            # handler 可能长时间运行并持有很多过期快照,提交一次再写终态,
            # 避免 REPEATABLE READ 下读到 handler 之前的旧数据
            db.expire_all()
            task.status = DONE
            task.result = result
            task.finished_at = datetime.now()
            db.commit()
            logger.info("任务完成 id=%s type=%s", task_id, task.type)
        except Exception as exc:  # noqa: BLE001
            logger.exception("任务失败 id=%s", task_id)
            try:
                db.rollback()
                task = db.get(Task, task_id)
                if task is not None:
                    task.status = FAILED
                    task.error = str(exc)[:4000]
                    task.finished_at = datetime.now()
                    db.commit()
            except Exception:  # noqa: BLE001
                logger.exception("标记任务失败状态时出错 id=%s", task_id)


def cancel_task(db: Session, task: Task) -> bool:
    """取消任务:仅 pending 可取消(running 的线程无法安全中断)。

    backtest 任务关联的 BacktestRun 一并标记 cancelled,避免滞留 pending。
    返回 False 表示任务已不在 pending(调用方转 409)。
    """
    res = db.execute(
        update(Task)
        .where(Task.id == task.id, Task.status == PENDING)
        .values(status=CANCELLED, finished_at=datetime.now())
    )
    if res.rowcount != 1:
        db.rollback()
        return False
    if task.type == "backtest" and task.ref_id is not None:
        db.execute(
            update(BacktestRun)
            .where(BacktestRun.id == task.ref_id, BacktestRun.status == PENDING)
            .values(
                status=CANCELLED, error="已取消", finished_at=datetime.now(),
            )
        )
    db.commit()
    return True


def recover_tasks() -> None:
    """启动恢复:running → failed(线程已随进程消失);pending 重新派发。"""
    with SessionLocal() as db:
        stale = db.execute(
            select(Task).where(Task.status == RUNNING)
        ).scalars().all()
        for task in stale:
            task.status = FAILED
            task.error = "服务重启,任务中断"
            task.finished_at = datetime.now()
            if task.type == "backtest" and task.ref_id is not None:
                db.execute(
                    update(BacktestRun)
                    .where(
                        BacktestRun.id == task.ref_id,
                        BacktestRun.status.in_((PENDING, RUNNING)),
                    )
                    .values(
                        status=FAILED, error="服务重启,任务中断",
                        finished_at=datetime.now(),
                    )
                )
        pending_ids = list(db.execute(
            select(Task.id).where(Task.status == PENDING)
        ).scalars())
        db.commit()
    if settings.task_async:
        for task_id in pending_ids:
            _get_executor().submit(run_task, task_id)
    if stale or pending_ids:
        logger.info(
            "任务恢复: %d 个中断标记失败, %d 个 pending 待重新派发",
            len(stale), len(pending_ids),
        )


def shutdown_tasks() -> None:
    if _executor is not None:
        _executor.shutdown(wait=False, cancel_futures=True)


def task_payload(task: Task, *, with_result: bool = False) -> dict[str, Any]:
    payload: dict[str, Any] = {
        "id": task.id,
        "type": task.type,
        "status": task.status,
        "title": task.title,
        "ref_id": task.ref_id,
        "params": task.params or {},
        "error": task.error,
        "created_at": (
            task.created_at.isoformat(sep=" ") if task.created_at else None
        ),
        "started_at": (
            task.started_at.isoformat(sep=" ") if task.started_at else None
        ),
        "finished_at": (
            task.finished_at.isoformat(sep=" ") if task.finished_at else None
        ),
    }
    if with_result:
        payload["result"] = task.result
    return payload


__all__ = [
    "ACTIVE_STATUSES", "CANCELLED", "DONE", "FAILED", "PENDING", "RUNNING",
    "HANDLERS", "TaskConflictError",
    "cancel_task", "recover_tasks", "register_handler", "run_task",
    "shutdown_tasks", "submit_task", "task_payload", "user_active_task",
]
