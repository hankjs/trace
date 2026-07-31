"""A2A Task 存储、配额、限速与长任务映射。

- ephemeral 短任务：内存 dict，TTL 15 分钟，Get 时清理过期。
- 长任务：以 quant_task / FactorEvaluation 等 DB 记录为准；A2A task id 采用
  ``quant_task_{id}`` 前缀，方便断线后 Get Task 恢复。
- 读/create 类限速：进程内滑动窗口 60/用户/分钟。
- 高成本日配额：以 quant_a2a_audit 当日记录数为准。
"""
from __future__ import annotations

import asyncio
import logging
import threading
from collections import defaultdict, deque
from datetime import date, datetime, timedelta
from typing import Any

from a2a.server.context import ServerCallContext
from a2a.server.tasks.task_store import TaskStore
from a2a.types.a2a_pb2 import (
    ListTasksRequest,
    ListTasksResponse,
    Task,
    TaskState,
    TaskStatus,
)
from google.protobuf import struct_pb2
from sqlalchemy import select
from sqlalchemy.orm import Session

from ..config import settings
from ..db import SessionLocal
from ..models import A2aAudit, BacktestRun, FactorEvaluation, Task as QuantTask

logger = logging.getLogger(__name__)

HIGH_COST_SKILLS = frozenset({
    "backtest.run",
    "experiment.trial",
    "experiment.trial_batch",
    "factor.evaluate",
})

# skill -> quant_task.type，用于互斥/幂等查找
SKILL_TO_QUANT_TASK_TYPE: dict[str, str] = {
    "backtest.run": "backtest",
    "experiment.trial": "experiment_trial",
    "experiment.trial_batch": "experiment_trial_batch",
    "factor.evaluate": "factor_evaluation",
}

_QUANT_TASK_TYPE_TO_SKILL = {v: k for k, v in SKILL_TO_QUANT_TASK_TYPE.items()}

_SHORT_TASK_TTL_MINUTES = settings.a2a_short_task_ttl_minutes


def _now() -> datetime:
    return datetime.now()


def _today_start() -> datetime:
    return datetime.combine(date.today(), datetime.min.time())


def _quant_status_to_a2a(status: str) -> TaskState:
    mapping = {
        "pending": TaskState.TASK_STATE_SUBMITTED,
        "running": TaskState.TASK_STATE_WORKING,
        "done": TaskState.TASK_STATE_COMPLETED,
        "failed": TaskState.TASK_STATE_FAILED,
        "cancelled": TaskState.TASK_STATE_CANCELED,
    }
    return mapping.get(status, TaskState.TASK_STATE_FAILED)


def _task_from_quant(quant: QuantTask) -> Task:
    """把 quant_task 行转成 A2A Task（无 artifacts，用于 List/Get）。"""
    task = Task(
        id=f"quant_task_{quant.id}",
        context_id="quant",
        status=TaskStatus(state=_quant_status_to_a2a(quant.status)),
    )
    if quant.error:
        task.status.message.text = quant.error
        task.status.message.role = 2  # ROLE_AGENT
    # 透传少量元数据，便于 client 识别
    meta = {
        "quant_task_id": quant.id,
        "quant_task_type": quant.type,
        "ref_id": quant.ref_id,
    }
    task.metadata.MergeFrom(struct_pb2.Value())
    task.metadata.MergeFrom(struct_pb2.Struct())  # type: ignore[arg-type]
    for k, v in meta.items():
        if v is not None:
            task.metadata[k] = v  # type: ignore[index]
    return task


def _db() -> Session:
    return SessionLocal()


class EphemeralTaskStore(TaskStore):
    """短任务内存存储 + 长任务 DB 映射。

    短任务完成/失败后仍保留 TTL 分钟，供 Get Task 碰运气；
    长任务只在内存保留映射，真实状态以 quant_task/FactorEvaluation 为准。
    """

    def __init__(self) -> None:
        # a2a_task_id -> (Task protobuf, expires_at)
        self._short: dict[str, tuple[Task, datetime]] = {}
        # a2a_task_id -> {"quant_task_id": int|None, "record_id": int|None, "skill": str}
        self._long_meta: dict[str, dict[str, Any]] = {}
        self._lock = asyncio.Lock()

    async def save(self, task: Task, context: ServerCallContext) -> None:
        async with self._lock:
            self._short[task.id] = (task, _now() + timedelta(minutes=_SHORT_TASK_TTL_MINUTES))

    def _cleanup_expired(self) -> None:
        now = _now()
        expired = [tid for tid, (_, exp) in self._short.items() if exp < now]
        for tid in expired:
            self._short.pop(tid, None)

    async def get(self, task_id: str, context: ServerCallContext) -> Task | None:
        # 长任务：以 DB 为准
        if task_id.startswith("quant_task_"):
            try:
                qid = int(task_id[len("quant_task_"):])
            except ValueError:
                return None
            with _db() as db:
                quant = db.get(QuantTask, qid)
            if quant is None:
                return None
            return _task_from_quant(quant)

        async with self._lock:
            self._cleanup_expired()
            entry = self._short.get(task_id)
            if entry is not None:
                return entry[0]
            meta = self._long_meta.get(task_id)
            if meta is None:
                return None
        # 内存映射存在但短任务缓存已被清理：从 DB 重建
        qid = meta.get("quant_task_id")
        if qid is not None:
            with _db() as db:
                quant = db.get(QuantTask, qid)
            if quant is not None:
                return _task_from_quant(quant)
        return None

    async def list(self, params: ListTasksRequest, context: ServerCallContext) -> ListTasksResponse:
        claims = context.state.get("claims")
        user_id = str(claims.get("sub")) if claims else ""
        if not user_id:
            return ListTasksResponse(tasks=[])
        with _db() as db:
            rows = db.execute(
                select(QuantTask)
                .where(QuantTask.user_id == user_id)
                .where(QuantTask.type.in_(list(SKILL_TO_QUANT_TASK_TYPE.values())))
                .order_by(QuantTask.id.desc())
                .limit(params.page_size or 50)
            ).scalars().all()
        tasks = [_task_from_quant(r) for r in rows]
        return ListTasksResponse(tasks=tasks)

    async def delete(self, task_id: str, context: ServerCallContext) -> None:
        async with self._lock:
            self._short.pop(task_id, None)
            self._long_meta.pop(task_id, None)

    def register_long_meta(
        self,
        a2a_task_id: str,
        *,
        quant_task_id: int | None = None,
        record_id: int | None = None,
        skill: str = "",
    ) -> None:
        """注册 A2A task id 与 DB 长任务记录的映射。"""
        self._long_meta[a2a_task_id] = {
            "quant_task_id": quant_task_id,
            "record_id": record_id,
            "skill": skill,
        }

    def get_long_meta(self, a2a_task_id: str) -> dict[str, Any] | None:
        return self._long_meta.get(a2a_task_id)


class RateLimiter:
    """进程内滑动窗口限速。"""

    def __init__(self, limit: int, window_seconds: int) -> None:
        self._limit = limit
        self._window = window_seconds
        self._windows: dict[str, deque[float]] = defaultdict(deque)

    def check(self, user_id: str) -> bool:
        now = datetime.now().timestamp()
        window = self._windows[user_id]
        cutoff = now - self._window
        while window and window[0] < cutoff:
            window.popleft()
        if len(window) >= self._limit:
            return False
        window.append(now)
        return True


class QuotaTracker:
    """高成本 skill 日配额追踪。"""

    def __init__(self, quota: int) -> None:
        self._quota = quota

    def remaining(self, user_id: str) -> int:
        start = _today_start()
        with _db() as db:
            used = db.execute(
                select(A2aAudit)
                .where(A2aAudit.user_id == user_id)
                .where(A2aAudit.created_at >= start)
                .where(A2aAudit.skill.in_(list(HIGH_COST_SKILLS)))
            ).scalars().all()
        return max(0, self._quota - len(used))

    def is_available(self, user_id: str, cost: int = 1) -> bool:
        return self.remaining(user_id) >= cost


def find_idempotent_task(user_id: str, client_request_id: str, skill: str) -> QuantTask | None:
    """同一用户当日相同 client_request_id 返回已存在的 quant_task。"""
    if not client_request_id:
        return None
    task_type = SKILL_TO_QUANT_TASK_TYPE.get(skill)
    if task_type is None:
        return None
    start = _today_start()
    with _db() as db:
        rows = db.execute(
            select(QuantTask)
            .where(QuantTask.user_id == user_id)
            .where(QuantTask.type == task_type)
            .where(QuantTask.created_at >= start)
            .order_by(QuantTask.id.desc())
            .limit(20)
        ).scalars().all()
    for row in rows:
        params = row.params or {}
        if params.get("client_request_id") == client_request_id:
            return row
    return None


def record_audit(
    user_id: str,
    a2a_task_id: str,
    skill: str,
    source: str,
    *,
    run_id: int | None = None,
    experiment_id: int | None = None,
    trial_id: int | None = None,
    failure_kind: str | None = None,
    missing_capability: str | None = None,
) -> None:
    """旁路写 audit；失败只记日志不阻断主流程。"""
    try:
        with _db() as db:
            db.add(
                A2aAudit(
                    user_id=user_id,
                    a2a_task_id=a2a_task_id,
                    skill=skill,
                    source=source,
                    run_id=run_id,
                    experiment_id=experiment_id,
                    trial_id=trial_id,
                    failure_kind=failure_kind,
                    missing_capability=missing_capability,
                    created_at=datetime.now(),
                )
            )
            db.commit()
    except Exception:  # noqa: BLE001
        logger.exception("A2A audit 写入失败")


def get_backtest_summary(run: BacktestRun) -> dict[str, Any]:
    """从 BacktestRun 行构造 backtest_summary artifact。"""
    metrics = run.metrics or {}
    validation = metrics.get("validation") or {}
    data_quality = metrics.get("data_quality") or {}
    return {
        "run_id": run.id,
        "strategy_id": run.strategy_id,
        "status": getattr(run, "status", None) or "done",
        "error": getattr(run, "error", None),
        "start": str(run.start),
        "end": str(run.end),
        "metrics": {
            "total_return": metrics.get("total_return"),
            "annual_return": metrics.get("annual_return"),
            "max_drawdown": metrics.get("max_drawdown"),
            "sharpe": metrics.get("sharpe"),
            "win_rate": metrics.get("win_rate"),
            "n_trades": metrics.get("trade_count"),
        },
        "validation": {
            "verdict": validation.get("rejection", {}).get("verdict", "incomplete"),
            "reasons": validation.get("rejection", {}).get("hits", []),
        },
        "data_quality": {
            "st_history_incomplete": data_quality.get("st_history_incomplete", False),
            "notes": data_quality.get("warnings", []),
        },
        "detail_ref": {"run_id": run.id},
    }


def get_factor_evaluation_summary(row: FactorEvaluation) -> dict[str, Any]:
    """从 FactorEvaluation 行构造 factor_evaluation artifact。"""
    result = row.result or {}
    ic = result.get("ic") or {}
    return {
        "evaluation_id": row.id,
        "factor_key": row.factor_key,
        "expression_hash": row.expression_hash,
        "universe": row.universe,
        "window": {
            "start": str(row.start),
            "end": str(row.end),
            "rebalance": row.rebalance,
        },
        "ic": {
            "ic_mean": ic.get("ic_mean"),
            "icir": ic.get("icir"),
            "rank_ic_mean": ic.get("rank_ic_mean"),
            "positive_ratio": ic.get("positive_ratio"),
            "n_periods": ic.get("n_periods"),
        },
        "layers": result.get("layers", []),
        "long_short": result.get("long_short", {}),
        "coverage": result.get("coverage", {}),
        "detail_ref": {"evaluation_id": row.id},
        "status": row.status,
        "error": row.error,
    }


# 全局实例（单实例假设）
short_task_store = EphemeralTaskStore()
rate_limiter = RateLimiter(limit=settings.a2a_read_rate_limit, window_seconds=60)
quota_tracker = QuotaTracker(quota=settings.a2a_daily_quota)

__all__ = [
    "HIGH_COST_SKILLS",
    "SKILL_TO_QUANT_TASK_TYPE",
    "EphemeralTaskStore",
    "QuotaTracker",
    "RateLimiter",
    "find_idempotent_task",
    "get_backtest_summary",
    "get_factor_evaluation_summary",
    "quota_tracker",
    "rate_limiter",
    "record_audit",
    "short_task_store",
]
