"""调度器跨进程互斥(REVIEW 问题 6 / brief §3.6)。

每个 FastAPI 实例都会拉起 APScheduler,而 fundamentals.py 的 threading.Lock
只在单进程内有效。多 worker / 多副本 / 滚动部署时会重复抓取写入。

这里断言 app/scheduler_lock.py 的三条分支各自的精确行为。
"""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app import scheduler_lock


class _FakeDialect:
    def __init__(self, name: str) -> None:
        self.name = name


class _FakeConnection:
    def __init__(self, lock_result, fail: bool = False) -> None:
        self._lock_result = lock_result
        self._fail = fail
        self.closed = False
        self.statements: list[str] = []

    def execute(self, statement, params=None):
        text = str(statement)
        self.statements.append(text)
        if self._fail:
            raise RuntimeError("连接中断")

        class _Result:
            def __init__(self, value):
                self._value = value

            def scalar(self):
                return self._value

        return _Result(self._lock_result)

    def close(self) -> None:
        self.closed = True


class _FakeEngine:
    def __init__(self, dialect_name: str, connection: _FakeConnection | None = None):
        self.dialect = _FakeDialect(dialect_name)
        self._connection = connection

    def connect(self) -> _FakeConnection:
        return self._connection


def _reset() -> None:
    scheduler_lock._lock_connection = None


def _enable_prod_scheduler(monkeypatch) -> None:
    """生产 + 开关开:后续用例只测互斥锁分支。"""
    monkeypatch.setattr(scheduler_lock.settings, "env", "prod")
    monkeypatch.setattr(scheduler_lock.settings, "scheduler_enabled", True)


def test_dev_env_skips_scheduler_even_when_enabled(monkeypatch):
    """本地/开发默认 env=dev:不启调度,避免日线与盘中同步。"""
    _reset()
    monkeypatch.setattr(scheduler_lock.settings, "env", "dev")
    monkeypatch.setattr(scheduler_lock.settings, "scheduler_enabled", True)

    def _boom():
        raise AssertionError("env=dev 时不应连接数据库抢锁")

    monkeypatch.setattr(scheduler_lock, "engine", _FakeEngine("mysql"))
    monkeypatch.setattr(scheduler_lock.engine, "connect", _boom)

    assert scheduler_lock.acquire_scheduler_slot() is False
    assert scheduler_lock._lock_connection is None


def test_disabled_by_config_does_not_even_connect(monkeypatch):
    """纯 API worker 可用开关彻底退出调度,不该去碰数据库。"""
    _reset()
    monkeypatch.setattr(scheduler_lock.settings, "env", "prod")
    monkeypatch.setattr(scheduler_lock.settings, "scheduler_enabled", False)

    def _boom():
        raise AssertionError("scheduler_enabled=False 时不应连接数据库")

    monkeypatch.setattr(scheduler_lock, "engine", _FakeEngine("mysql"))
    monkeypatch.setattr(scheduler_lock.engine, "connect", _boom)

    assert scheduler_lock.acquire_scheduler_slot() is False
    assert scheduler_lock._lock_connection is None


def test_non_mysql_dialect_runs_without_advisory_lock(monkeypatch):
    """sqlite 无 GET_LOCK,单进程测试环境退化为「开关为真即运行」。"""
    _reset()
    _enable_prod_scheduler(monkeypatch)
    monkeypatch.setattr(scheduler_lock, "engine", _FakeEngine("sqlite"))

    assert scheduler_lock.acquire_scheduler_slot() is True
    assert scheduler_lock._lock_connection is None  # 无锁可持


def test_mysql_instance_that_wins_lock_holds_the_connection(monkeypatch):
    """抢到锁:返回 True,并**保持连接引用**(GET_LOCK 是会话级的)。"""
    _reset()
    connection = _FakeConnection(lock_result=1)
    _enable_prod_scheduler(monkeypatch)
    monkeypatch.setattr(
        scheduler_lock, "engine", _FakeEngine("mysql", connection))

    assert scheduler_lock.acquire_scheduler_slot() is True
    # 连接必须不被关闭,否则锁立即释放,互斥形同虚设
    assert connection.closed is False
    assert scheduler_lock._lock_connection is connection
    assert any("GET_LOCK" in s for s in connection.statements)


def test_mysql_instance_that_loses_lock_closes_connection(monkeypatch):
    """没抢到锁:返回 False 且必须归还连接,否则连接池会被泄漏耗尽。"""
    _reset()
    connection = _FakeConnection(lock_result=0)
    _enable_prod_scheduler(monkeypatch)
    monkeypatch.setattr(
        scheduler_lock, "engine", _FakeEngine("mysql", connection))

    assert scheduler_lock.acquire_scheduler_slot() is False
    assert connection.closed is True
    assert scheduler_lock._lock_connection is None


def test_lock_failure_does_not_block_process_startup(monkeypatch):
    """拿锁报错不应让整个进程起不来,只是本实例不调度。"""
    _reset()
    connection = _FakeConnection(lock_result=None, fail=True)
    _enable_prod_scheduler(monkeypatch)
    monkeypatch.setattr(
        scheduler_lock, "engine", _FakeEngine("mysql", connection))

    assert scheduler_lock.acquire_scheduler_slot() is False
    assert connection.closed is True
    assert scheduler_lock._lock_connection is None


def test_release_is_idempotent_and_closes_connection(monkeypatch):
    """释放后重复调用不报错(优雅关闭路径可能被走两次)。"""
    _reset()
    connection = _FakeConnection(lock_result=1)
    _enable_prod_scheduler(monkeypatch)
    monkeypatch.setattr(
        scheduler_lock, "engine", _FakeEngine("mysql", connection))
    assert scheduler_lock.acquire_scheduler_slot() is True

    scheduler_lock.release_scheduler_slot()
    assert connection.closed is True
    assert any("RELEASE_LOCK" in s for s in connection.statements)
    assert scheduler_lock._lock_connection is None

    scheduler_lock.release_scheduler_slot()  # 第二次是 no-op,不应抛
    assert scheduler_lock._lock_connection is None
