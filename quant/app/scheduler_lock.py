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

会话级锁的代价:持锁连接空闲超过 MySQL `wait_timeout`(线上 3600s)会被
服务端杀掉。而周级/月级任务之间的空窗远超这个值,所以「连接还活着」
不能等同于「本实例还是 leader」,反之更要紧 —— 连接被回收**不等于**
锁被别人抢走。`ensure_scheduler_slot` 负责区分这两种情况,
`start_scheduler` 另挂一个保活任务定期探活,避免锁连接被空闲回收。

锁的获取放在 `app/main.py` 的 lifespan 里;`app/scheduler.py` 只在任务
执行前与保活任务里调用 `ensure_scheduler_slot`。
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


def _try_get_lock() -> bool:
    """连一条新连接抢 GET_LOCK;成功则把连接存到 `_lock_connection`。"""
    global _lock_connection

    try:
        connection = engine.connect()
    except Exception:  # noqa: BLE001 - 库不可达时不参与调度,但别把进程/任务打挂
        logger.exception("连接数据库失败,无法获取调度器互斥锁")
        return False
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
    return True


def acquire_scheduler_slot() -> bool:
    """本实例是否应该运行定时任务。"""
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

    if not _try_get_lock():
        return False
    logger.info("已取得调度器互斥锁 %s,本实例负责运行定时任务", LOCK_NAME)
    return True


def _drop_lock_connection() -> None:
    """丢弃已失效的持锁连接引用(不再发语句,连接已不可用)。"""
    global _lock_connection

    connection, _lock_connection = _lock_connection, None
    if connection is None:
        return
    try:
        # 事务可能处于 PendingRollback,先回滚再归还,否则 close 路径会抛
        connection.rollback()
    except Exception:  # noqa: BLE001 - 已失效的连接,回滚失败无所谓
        pass
    try:
        connection.close()
    except Exception:  # noqa: BLE001
        pass


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


def ensure_scheduler_slot() -> bool:
    """任务执行前/保活时确认本实例仍是 leader,必要时重新抢锁。

    关键区分(此前把两者混为一谈,导致整个实例调度停摆):
    - 锁**被其他实例抢走** -> 本实例必须停止调度;
    - 持锁连接被 MySQL 按 `wait_timeout` 回收(线上 3600s,而周级任务
      之间空窗远超此值)-> 只是连接没了,leader 身份并没有让给谁,
      重新 GET_LOCK 抢回来即可继续调度。

    返回 True 表示可以继续跑定时任务。
    """
    if _dialect_name() != "mysql":
        return True
    if is_scheduler_slot_held():
        return True
    # 走到这里:连接失效或锁不在本连接上。丢掉旧引用后重抢一次,
    # 抢不到才说明锁真的归了别人。
    _drop_lock_connection()
    if _try_get_lock():
        logger.warning(
            "持锁连接已失效(疑似 MySQL 空闲回收),已重新取得互斥锁 %s,继续调度",
            LOCK_NAME,
        )
        return True
    return False


def ping_scheduler_lock() -> bool:
    """保活探测:定期跑一次,避免持锁连接被 MySQL 按空闲超时回收。"""
    return ensure_scheduler_slot()


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
        # 连接已被服务端回收时这里必然失败(锁也早已随连接释放),
        # 记 info 即可:关闭路径打 exception 栈只会污染日志。
        logger.info("释放调度器互斥锁时连接已失效,锁已随连接自动释放")
        try:
            connection.rollback()
        except Exception:  # noqa: BLE001
            pass
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
