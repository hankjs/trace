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
        self.rolled_back = False
        self.statements: list[str] = []

    def execute(self, statement, params=None):
        if self.closed:
            raise RuntimeError("连接已关闭")
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

    def rollback(self) -> None:
        self.rolled_back = True

    def close(self) -> None:
        self.closed = True


class _FakeEngine:
    def __init__(self, dialect_name: str, connection: _FakeConnection | None = None,
                 connections: list[_FakeConnection] | None = None):
        self.dialect = _FakeDialect(dialect_name)
        self._connection = connection
        # 多次 connect 依次返回不同连接:用于「旧连接失效后重新抢锁」的场景
        self._queue = list(connections or [])
        self.connect_count = 0

    def connect(self) -> _FakeConnection:
        self.connect_count += 1
        if self._queue:
            return self._queue.pop(0)
        return self._connection


def _reset() -> None:
    scheduler_lock._lock_connection = None
    scheduler_lock._job_lock_connections.clear()


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


def test_scheduler_slot_held_checks_used_lock_on_same_connection(monkeypatch):
    """持有锁时 is_scheduler_slot_held 为 True;其它连接持有则为 False。"""
    _reset()
    conn = _FakeConnection(lock_result=1)
    _enable_prod_scheduler(monkeypatch)
    monkeypatch.setattr(scheduler_lock, "engine", _FakeEngine("mysql", conn))

    assert scheduler_lock.acquire_scheduler_slot() is True
    assert scheduler_lock.is_scheduler_slot_held() is True
    assert any("IS_USED_LOCK" in s for s in conn.statements)

    # 模拟连接断开
    conn.closed = True
    assert scheduler_lock.is_scheduler_slot_held() is False


def test_acquire_job_lock_uses_mysql_get_lock(monkeypatch):
    """手动触发任务使用 per-job GET_LOCK 跨 worker 互斥。"""
    _reset()
    conn = _FakeConnection(lock_result=1)
    _enable_prod_scheduler(monkeypatch)
    monkeypatch.setattr(scheduler_lock, "engine", _FakeEngine("mysql", conn))

    acquired = scheduler_lock.acquire_job_lock("sync_stock_list")
    assert acquired is conn
    assert "sync_stock_list" in scheduler_lock._job_lock_connections
    assert any("GET_LOCK" in s for s in conn.statements)

    scheduler_lock.release_job_lock("sync_stock_list")
    assert conn.closed is True
    assert "sync_stock_list" not in scheduler_lock._job_lock_connections


def test_acquire_job_lock_returns_none_when_already_held(monkeypatch):
    """同任务锁被其它实例持有时返回 None。"""
    _reset()
    conn = _FakeConnection(lock_result=0)
    _enable_prod_scheduler(monkeypatch)
    monkeypatch.setattr(scheduler_lock, "engine", _FakeEngine("mysql", conn))

    assert scheduler_lock.acquire_job_lock("sync_stock_list") is None
    assert conn.closed is True


def test_acquire_job_lock_noop_on_non_mysql(monkeypatch):
    """SQLite 等方言无 advisory lock,返回 True 由调用方做进程内互斥。"""
    _reset()
    _enable_prod_scheduler(monkeypatch)
    monkeypatch.setattr(scheduler_lock, "engine", _FakeEngine("sqlite"))

    assert scheduler_lock.acquire_job_lock("sync_stock_list") is True
    assert scheduler_lock.release_job_lock("sync_stock_list") is None


# ---- 空闲回收:连接没了 != 锁归了别人 ----
#
# 线上 wait_timeout=3600s,而周级/月级任务之间的空窗远超此值。旧实现
# 直接用 is_scheduler_slot_held 判定,连接被回收就当成失锁并一次性
# stop_scheduler,此后该实例**所有**定时任务永久停摆(journal 实证:
# 08-01 00:00 与 08-02 16:00 两次「互斥锁已丢失,停止本实例调度」)。


def test_ensure_slot_reacquires_after_idle_connection_reclaimed(monkeypatch):
    """持锁连接被服务端回收:应重新抢锁并继续调度,而不是判定失锁。"""
    _reset()
    dead = _FakeConnection(lock_result=1)
    fresh = _FakeConnection(lock_result=1)
    _enable_prod_scheduler(monkeypatch)
    engine = _FakeEngine("mysql", connections=[dead, fresh])
    monkeypatch.setattr(scheduler_lock, "engine", engine)

    assert scheduler_lock.acquire_scheduler_slot() is True
    dead.closed = True  # MySQL 按 wait_timeout 杀掉空闲连接

    assert scheduler_lock.ensure_scheduler_slot() is True
    # 必须换到新连接上重新持锁,且旧连接被归还(否则连接池泄漏)
    assert scheduler_lock._lock_connection is fresh
    assert engine.connect_count == 2
    assert any("GET_LOCK" in s for s in fresh.statements)


def test_ensure_slot_returns_false_when_lock_truly_taken(monkeypatch):
    """重抢失败才说明锁真的归了别人 —— 这时才该停调度。"""
    _reset()
    dead = _FakeConnection(lock_result=1)
    taken = _FakeConnection(lock_result=0)
    _enable_prod_scheduler(monkeypatch)
    monkeypatch.setattr(
        scheduler_lock, "engine",
        _FakeEngine("mysql", connections=[dead, taken]))

    assert scheduler_lock.acquire_scheduler_slot() is True
    dead.closed = True

    assert scheduler_lock.ensure_scheduler_slot() is False
    assert scheduler_lock._lock_connection is None
    assert taken.closed is True


def test_ensure_slot_is_noop_while_lock_healthy(monkeypatch):
    """连接健康时不该反复重连抢锁。"""
    _reset()
    conn = _FakeConnection(lock_result=1)
    _enable_prod_scheduler(monkeypatch)
    engine = _FakeEngine("mysql", conn)
    monkeypatch.setattr(scheduler_lock, "engine", engine)

    assert scheduler_lock.acquire_scheduler_slot() is True
    assert scheduler_lock.ensure_scheduler_slot() is True
    assert engine.connect_count == 1  # 没有多余的连接
    assert scheduler_lock._lock_connection is conn


def test_ensure_slot_rolls_back_broken_connection_before_discarding(monkeypatch):
    """失效连接上可能挂着 PendingRollback 事务,归还前必须先 rollback。"""
    _reset()
    dead = _FakeConnection(lock_result=1)
    fresh = _FakeConnection(lock_result=1)
    _enable_prod_scheduler(monkeypatch)
    monkeypatch.setattr(
        scheduler_lock, "engine",
        _FakeEngine("mysql", connections=[dead, fresh]))

    assert scheduler_lock.acquire_scheduler_slot() is True
    dead._fail = True  # 连接仍"开着"但每条语句都炸(pymysql 断连的典型表现)

    assert scheduler_lock.ensure_scheduler_slot() is True
    assert dead.rolled_back is True
    assert dead.closed is True


def test_ping_keeps_lock_alive_and_reports_loss(monkeypatch):
    """保活探测复用 ensure 语义:能重抢即 True,锁被抢走才 False。"""
    _reset()
    conn = _FakeConnection(lock_result=1)
    _enable_prod_scheduler(monkeypatch)
    monkeypatch.setattr(scheduler_lock, "engine", _FakeEngine("mysql", conn))

    assert scheduler_lock.acquire_scheduler_slot() is True
    assert scheduler_lock.ping_scheduler_lock() is True

    _reset()
    dead = _FakeConnection(lock_result=1)
    taken = _FakeConnection(lock_result=0)
    monkeypatch.setattr(
        scheduler_lock, "engine",
        _FakeEngine("mysql", connections=[dead, taken]))
    assert scheduler_lock.acquire_scheduler_slot() is True
    dead.closed = True
    assert scheduler_lock.ping_scheduler_lock() is False


def test_ensure_slot_survives_database_outage(monkeypatch):
    """库整体不可达时 ensure 返回 False,但不能抛异常打挂调度线程。"""
    _reset()
    dead = _FakeConnection(lock_result=1)
    _enable_prod_scheduler(monkeypatch)

    class _BrokenEngine(_FakeEngine):
        def connect(self):
            self.connect_count += 1
            raise RuntimeError("数据库不可达")

    engine = _BrokenEngine("mysql", dead)
    monkeypatch.setattr(scheduler_lock, "engine", engine)
    scheduler_lock._lock_connection = dead
    dead.closed = True

    assert scheduler_lock.ensure_scheduler_slot() is False


def test_release_does_not_raise_when_connection_already_reclaimed(monkeypatch):
    """连接已被回收时,优雅关闭路径不该抛(锁早已随连接释放)。"""
    _reset()
    conn = _FakeConnection(lock_result=1)
    _enable_prod_scheduler(monkeypatch)
    monkeypatch.setattr(scheduler_lock, "engine", _FakeEngine("mysql", conn))
    assert scheduler_lock.acquire_scheduler_slot() is True

    conn._fail = True  # RELEASE_LOCK 会炸(线上 PendingRollbackError)
    scheduler_lock.release_scheduler_slot()

    assert conn.closed is True
    assert scheduler_lock._lock_connection is None
