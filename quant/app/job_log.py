"""定时任务执行日志(quant_job_run)的读写。

系统调度监听器(app/scheduler.py)与 admin 手动触发(app/api/admin.py)
共用本模块。这是旁路日志:任何写库失败只记 logger,绝不让日志问题
影响任务本身的执行。
"""
from __future__ import annotations

import json
import logging
from datetime import datetime

from sqlalchemy import func, select
from sqlalchemy.orm import Session

from .db import SessionLocal
from .models import JobRun

logger = logging.getLogger(__name__)

TRIGGER_SYSTEM = "system"
TRIGGER_MANUAL = "manual"

STATUS_RUNNING = "running"
STATUS_FINISHED = "finished"
STATUS_FAILED = "failed"


def _dump_result(result: object) -> str | None:
    """任务返回值统一序列化为 JSON 文本;含 date 等对象时降级为字符串。"""
    if result is None:
        return None
    try:
        return json.dumps(result, ensure_ascii=False, default=str)
    except (TypeError, ValueError):
        return str(result)


def to_dict(row: JobRun) -> dict:
    result: object = None
    if row.result:
        try:
            result = json.loads(row.result)
        except ValueError:
            result = row.result
    return {
        "id": row.id,
        "job_id": row.job_id,
        "trigger": row.trigger,
        "status": row.status,
        "operator": row.operator,
        "started_at": row.started_at.isoformat(timespec="seconds"),
        "finished_at": (row.finished_at.isoformat(timespec="seconds")
                        if row.finished_at else None),
        "result": result,
        "error": row.error,
    }


def record_run(job_id: str, trigger: str, status: str,
               started_at: datetime, finished_at: datetime | None = None,
               operator: str | None = None, result: object = None,
               error: str | None = None) -> int | None:
    """写入一条执行记录,返回行 id;写库失败返回 None(不抛)。"""
    try:
        with SessionLocal() as db:
            row = JobRun(
                job_id=job_id, trigger=trigger, status=status,
                operator=operator, started_at=started_at,
                finished_at=finished_at, result=_dump_result(result),
                error=error,
            )
            db.add(row)
            db.commit()
            return row.id
    except Exception:  # noqa: BLE001 - 旁路日志不影响任务
        logger.exception("任务执行日志写入失败 %s", job_id)
        return None


def finish_run(run_id: int | None, status: str, finished_at: datetime,
               result: object = None, error: str | None = None) -> None:
    """更新手动触发时预写的 running 行为完成态。"""
    if run_id is None:
        return
    try:
        with SessionLocal() as db:
            row = db.get(JobRun, run_id)
            if row is None:
                return
            row.status = status
            row.finished_at = finished_at
            row.result = _dump_result(result)
            row.error = error
            db.commit()
    except Exception:  # noqa: BLE001
        logger.exception("任务执行日志更新失败 run_id=%s", run_id)


def fail_stale_running(job_id: str, trigger: str = TRIGGER_MANUAL) -> None:
    """同一任务新一次执行前,把遗留的 running 行标记为失败。

    进程在任务执行中途崩溃/重启会留下永远 running 的行,下次触发时
    统一收尾,避免页面上出现永不结束的「执行中」。
    """
    try:
        with SessionLocal() as db:
            rows = db.execute(
                select(JobRun).where(
                    JobRun.job_id == job_id,
                    JobRun.trigger == trigger,
                    JobRun.status == STATUS_RUNNING,
                )
            ).scalars().all()
            for row in rows:
                row.status = STATUS_FAILED
                row.error = "进程中断,执行未完成"
            if rows:
                db.commit()
    except Exception:  # noqa: BLE001
        logger.exception("遗留 running 日志收尾失败 %s", job_id)


def latest_runs() -> dict[tuple[str, str], dict]:
    """每个 (job_id, trigger) 的最近一条记录;读失败返回空(页面降级)。"""
    try:
        with SessionLocal() as db:
            latest_ids = select(func.max(JobRun.id)).group_by(
                JobRun.job_id, JobRun.trigger)
            rows = db.execute(
                select(JobRun).where(JobRun.id.in_(latest_ids))
            ).scalars().all()
            return {(r.job_id, r.trigger): to_dict(r) for r in rows}
    except Exception:  # noqa: BLE001
        logger.exception("任务执行日志读取失败")
        return {}


def recent_runs(db: Session, job_id: str, limit: int = 20) -> list[dict]:
    """单个任务的执行历史,新到旧。"""
    rows = db.execute(
        select(JobRun).where(JobRun.job_id == job_id)
        .order_by(JobRun.id.desc()).limit(limit)
    ).scalars().all()
    return [to_dict(r) for r in rows]
