"""pytest 全局夹具:测试默认同步回测,避免 BackgroundTasks 异步干扰断言。"""
from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.config import settings


@pytest.fixture(autouse=True)
def _sync_backtest_for_tests():
    previous = settings.backtest_async
    settings.backtest_async = False
    yield
    settings.backtest_async = previous


@pytest.fixture(autouse=True)
def _sync_tasks_for_tests():
    """任务系统默认同步内联执行:submit_task 返回时任务已到终态。"""
    previous = settings.task_async
    settings.task_async = False
    yield
    settings.task_async = previous
