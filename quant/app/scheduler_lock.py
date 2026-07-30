"""调度器跨进程互斥。

问题(REVIEW 问题 6):每个 FastAPI 实例都在启动时拉起 APScheduler,
而 `app/data/fundamentals.py:33` 的 `threading.Lock` 只在单进程内有效。
多 worker / 多副本 / 滚动部署时会重复抓取写入 —— 快照无唯一键会重复落行、
选股 delete/insert 互相竞争、评估产出重复批次。

方案(见 logs/decisions-migrate.md D8):
1. 仅 production 环境允许调度 —— dev/local 只跑业务 API,避免本地
   reload 触发 baostock/akshare 日线与盘中同步、污染共享库;
2. `settings.scheduler_enabled` 粗开关 —— 纯 API worker 可置 false 完全不参与调度;
3. 开关为真时再抢 MySQL 会话级 advisory lock `GET_LOCK(name, 0)`,
   只有抢到的那个实例真正运行定时任务。锁随连接生命周期释放,
   实例崩溃不会留下死锁(不同于表里插一行「我是 leader」的做法)。
4. 非 MySQL 方言(sqlite 测试环境)无 GET_LOCK,退化为「开关为真即运行」——
   单进程测试不存在跨进程竞态。

本模块刻意独立于 `app/scheduler.py`:那个文件属 agent-data 的 scope
(见 logs/scope-gap.md 5.1),锁的获取放在 `app/main.py` 的 lifespan 里,
`scheduler.py` 一行不用改。
"""
from __future__ import annotations

import logging
from typing import Any

from sqlalchemy import text

from .config import ENV_PROD, settings
from .db import engine

logger = logging.getLogger(__name__)

LOCK_NAME = "quant_scheduler"

# 持有锁的连接。必须保持引用:GET_LOCK 是会话级的,连接一还池/关闭锁就没了。
_lock_connection = None

# 手动触发任务的 per-job 锁连接(同 scheduler_lock 的 MySQL 会话锁语义)。
_job_lock_connections: dict[str, Any] = {}


def _dialect_name() -> str:
    return engine.dialect.name


def acquire_scheduler_slot() -> bool:
    """本实例是否应该运行定时任务。"""
    global _lock_connection

    if settings.env != ENV_PROD:
        logger.info(
            "env=%s,非生产环境不启动调度器"
            "(日线/盘中/估值等定时任务仅 production 运行)",
            settings.env,
        )
        return False

    if not settings.scheduler_enabled:
        logger.info("scheduler_enabled=false,本实例不参与调度")
        return False

    if _dialect_name() != "mysql":
        # sqlite/其他:无 advisory lock,单进程场景直接放行
        logger.info("非 MySQL 方言(%s),跳过跨进程互斥", engine.dialect.name)
        return True

    connection = engine.connect()
    try:
        acquired = connection.execute(
            text("SELECT GET_LOCK(:name, 0)"), {"name": LOCK_NAME},
        ).scalar()
    except Exception:  # noqa: BLE001 - 拿锁失败不应阻断整个进程启动
        connection.close()
        logger.exception("获取调度器互斥锁失败,本实例不参与调度")
        return False

    if acquired != 1:
        connection.close()
        logger.info("调度器互斥锁已被其他实例持有,本实例不参与调度")
        return False

    _lock_connection = connection
    logger.info("已取得调度器互斥锁 %s,本实例负责运行定时任务", LOCK_NAME)
    return True


def is_scheduler_slot_held() -> bool:
    """校验本实例是否仍持有调度器互斥锁(连接断开后锁会自动释放)。"""
    if _dialect_name() != "mysql":
        return True
    if _lock_connection is None:
        return False
    try:
        holder = _lock_connection.execute(
            text("SELECT IS_USED_LOCK(:name)"), {"name": LOCK_NAME},
        ).scalar()
        me = _lock_connection.execute(text("SELECT CONNECTION_ID()")).scalar()
    except Exception:  # noqa: BLE001 - 连接已断开即视为失锁
        return False
    return holder is not None and holder == me


def ping_scheduler_lock() -> bool:
    """对持有锁的连接做一次心跳探测;失锁时返回 False 并清空引用。"""
    global _lock_connection
    if _dialect_name() != "mysql":
        return True
    if _lock_connection is None:
        return False
    try:
        _lock_connection.execute(text("SELECT 1"))
    except Exception:  # noqa: BLE001
        _lock_connection = None
        return False
    return is_scheduler_slot_held()


def release_scheduler_slot() -> None:
    """释放锁(优雅关闭时调用;进程崩溃时由连接断开自动释放)。"""
    global _lock_connection

    connection, _lock_connection = _lock_connection, None
    if connection is None:
        return
    try:
        connection.execute(text("SELECT RELEASE_LOCK(:name)"), {"name": LOCK_NAME})
        logger.info("已释放调度器互斥锁 %s", LOCK_NAME)
    except Exception:  # noqa: BLE001 - 关闭路径不抛
        logger.exception("释放调度器互斥锁失败")
    finally:
        connection.close()


def _job_lock_name(job_id: str) -> str:
    return f"quant_job_{job_id}"


def acquire_job_lock(job_id: str, *, blocking: bool = False) -> Any:
    """为手动触发任务获取 MySQL 会话级互斥锁。

    非 MySQL 方言返回 True(由调用方自行决定进程内互斥方案);
    获取成功返回连接对象,失败返回 None。注意返回的连接对象必须被
    `release_job_lock` 释放,否则锁会持续到连接断开。
    """
    if _dialect_name() != "mysql":
        return True
    connection = engine.connect()
    timeout = -1 if blocking else 0
    try:
        acquired = connection.execute(
            text("SELECT GET_LOCK(:name, :timeout)"),
            {"name": _job_lock_name(job_id), "timeout": timeout},
        ).scalar()
    except Exception:  # noqa: BLE001
        connection.close()
        logger.exception("获取任务 %s 互斥锁失败", job_id)
        return None
    if acquired == 1:
        _job_lock_connections[job_id] = connection
        return connection
    connection.close()
    return None


def release_job_lock(job_id: str) -> None:
    """释放手动触发任务的 MySQL 会话锁。"""
    if _dialect_name() != "mysql":
        return
    connection = _job_lock_connections.pop(job_id, None)
    if connection is None:
        return
    try:
        connection.execute(
            text("SELECT RELEASE_LOCK(:name)"),
            {"name": _job_lock_name(job_id)},
        )
    except Exception:  # noqa: BLE001
        logger.exception("释放任务 %s 互斥锁失败", job_id)
    finally:
        connection.close()
