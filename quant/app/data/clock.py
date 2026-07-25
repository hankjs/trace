"""统一时钟:全项目对齐 Asia/Shanghai。

数据库时间列为 naive DateTime,历史数据按「上海本地时间」语义写入,
因此这里统一提供去掉 tzinfo 的上海时间,避免在非 CST 主机上
盘中门限、快照时间戳与 cleanup 保留窗口互相错位(REVIEW §3.5 时区不一致)。
"""
from __future__ import annotations

from datetime import date, datetime
from zoneinfo import ZoneInfo

SHANGHAI_TZ = ZoneInfo("Asia/Shanghai")


def now_cst() -> datetime:
    """带时区的上海当前时间"""
    return datetime.now(SHANGHAI_TZ)


def naive_now_cst() -> datetime:
    """上海当前时间(去 tzinfo),用于写入 naive DateTime 列"""
    return now_cst().replace(tzinfo=None)


def today_cst() -> date:
    """上海当前日期。替代裸 date.today()"""
    return now_cst().date()
