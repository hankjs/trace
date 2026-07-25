"""在长超时会话中执行 quant_daily_bar 的自然主键改造(0004 的 DDL)。

## 为什么需要绕过 Alembic 直接执行

腾讯云 CDB 的 `net_read_timeout` 默认 30 秒。`0004_daily_bar_natural_pk`
把四个操作合成一条 `ALTER TABLE`,在 1110 万行的表上重建整表远超 30 秒,
于是客户端被断开:

    pymysql (2013, 'Lost connection to MySQL server during query')

MySQL 的 `ALTER TABLE` 是原子的,服务端会回滚,表结构与数据无损 —— 但
Alembic 也就永远推进不过这一步。这里用 `read_timeout=7200` 的独立连接
执行同一条 DDL,完成后再 `alembic stamp 0004` 让版本对齐。

## 幂等性

执行前检查主键:若已是 `(code, date)` 则直接跳过。可安全重跑。

用法:
    uv run python scripts/apply_daily_bar_pk.py            # 只检查
    uv run python scripts/apply_daily_bar_pk.py --apply    # 实际执行
"""
from __future__ import annotations

import argparse
import sys
import time
from pathlib import Path
from urllib.parse import unquote, urlparse

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import pymysql  # noqa: E402

from app.config import settings  # noqa: E402

TIMEOUT = 7200

# 与 alembic/versions/0004_daily_bar_natural_pk.py 的 upgrade() 等价。
DDL = (
    "ALTER TABLE `quant_daily_bar` "
    "DROP PRIMARY KEY, "
    "DROP COLUMN `id`, "
    "DROP INDEX `uq_daily_bar_code_date`, "
    "DROP INDEX `ix_quant_daily_bar_code`, "
    "ADD PRIMARY KEY (`code`, `date`)"
)


def connect() -> pymysql.connections.Connection:
    u = urlparse(settings.database_url.replace("mysql+pymysql://", "mysql://"))
    return pymysql.connect(
        host=u.hostname, port=u.port or 3306, user=u.username,
        password=unquote(u.password or ""), database=u.path.lstrip("/"),
        read_timeout=TIMEOUT, write_timeout=TIMEOUT, connect_timeout=60,
        autocommit=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(description="长超时会话执行 0004 的 DDL")
    parser.add_argument("--apply", action="store_true", help="实际执行(默认只检查)")
    args = parser.parse_args()

    cn = connect()
    try:
        with cn.cursor() as cur:
            cur.execute(
                "SELECT COLUMN_NAME FROM information_schema.KEY_COLUMN_USAGE "
                "WHERE TABLE_SCHEMA = DATABASE() "
                "AND TABLE_NAME = 'quant_daily_bar' "
                "AND CONSTRAINT_NAME = 'PRIMARY' ORDER BY ORDINAL_POSITION"
            )
            pk = [r[0] for r in cur.fetchall()]
            cur.execute("SELECT COUNT(*) FROM `quant_daily_bar`")
            rows = cur.fetchone()[0]

        print(f"当前主键: {pk}")
        print(f"行数: {rows:,}")

        if pk == ["code", "date"]:
            print("\n✅ 主键已是 (code, date),无需执行。"
                  "若 alembic 版本仍落后,执行 alembic stamp 0004_daily_bar_natural_pk")
            return

        if not args.apply:
            print(f"\n[试运行] 将执行:\n  {DDL}")
            print(f"\n会话超时设为 {TIMEOUT}s。确认后加 --apply 执行。")
            return

        print(f"\n执行中(1100 万行重建表,预计数分钟,超时 {TIMEOUT}s)...")
        t0 = time.time()
        with cn.cursor() as cur:
            cur.execute(
                f"SET SESSION net_read_timeout={TIMEOUT}, "
                f"net_write_timeout={TIMEOUT}"
            )
            cur.execute(DDL)
        print(f"✅ 完成,耗时 {time.time() - t0:.0f}s")

        with cn.cursor() as cur:
            cur.execute(
                "SELECT COLUMN_NAME FROM information_schema.KEY_COLUMN_USAGE "
                "WHERE TABLE_SCHEMA = DATABASE() "
                "AND TABLE_NAME = 'quant_daily_bar' "
                "AND CONSTRAINT_NAME = 'PRIMARY' ORDER BY ORDINAL_POSITION"
            )
            print("新主键:", [r[0] for r in cur.fetchall()])
            cur.execute("SELECT COUNT(*) FROM `quant_daily_bar`")
            after = cur.fetchone()[0]
            print(f"行数: {after:,}" + ("" if after == rows else f"  ⚠️ 原为 {rows:,}"))

        print("\n接下来: uv run alembic stamp 0004_daily_bar_natural_pk")
        print("        uv run alembic upgrade 0005")
    finally:
        cn.close()


if __name__ == "__main__":
    main()
