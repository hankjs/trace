"""Skill handler 公共类型与工具。"""
from __future__ import annotations

import threading
import time
from dataclasses import dataclass
from typing import Any, Callable

from sqlalchemy.orm import Session

from ...tasks import cancel_task


@dataclass
class A2AContext:
    """单个 A2A skill 调用时的上下文。"""

    user_id: str
    claims: dict
    source: str
    a2a_task_id: str
    db: Session


SkillHandler = Callable[[dict[str, Any], A2AContext, threading.Event | None], dict[str, Any]]


def wait_for_task(db: Session, task, cancel_event: threading.Event | None) -> None:
    """轮询 quant_task 到终态；A2A 侧取消时触发协作取消（pending 即取消，
    running 置位取消事件后由引擎检查点在下一交易日/标的批次安全退出，
    这里继续等到状态翻转为止）。"""
    cancel_requested = False
    while task.status in {"pending", "running"}:
        if cancel_event is not None and cancel_event.is_set() and not cancel_requested:
            cancel_task(db, task)
            cancel_requested = True
        time.sleep(0.5)
        db.refresh(task)


__all__ = ["A2AContext", "SkillHandler", "wait_for_task"]
