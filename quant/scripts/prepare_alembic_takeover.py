#!/usr/bin/env python
"""Alembic 接管前的生产库状态校正(幂等,可重复执行)。

## 为什么需要这个脚本

生产库在 Alembic 引入前被旧的 `Base.metadata.create_all()` 建过表
(`scripts/backfill_pool.py` 曾在每次回填时调用它,已移除),导致库处于
**半新半旧的混合状态**:

- `alembic_version` 表不存在(Alembic 从未接管);
- `quant_pool` / `quant_pool_member` / `quant_trade_calendar` **已存在但列不全**
  (按当时的 models 建的,例如 `quant_pool` 缺 `created_at`、
  `quant_trade_calendar` 缺 `source`);
- `quant_stock` 缺 `list_date`/`delist_date`/`is_st`;
- `quant_backtest_run` 缺 `costs`/`pool_id`;
- `quant_daily_bar` 主键仍是 `id`,`user_id` 仍是 `BIGINT`。

这个状态下两条路都不通:
- 直接 `alembic upgrade head` → `0005` 的 `create_table` 撞「表已存在」;
- `alembic stamp` 了事 → 跳过真正需要执行的 ALTER,留下残缺 schema。

## 这个脚本做什么

只删除那几张**误建且为空**的表,让 `0005` 能从干净状态建出完整定义。
非空则拒绝删除并报错退出 —— 宁可停下让人确认,也不误删数据。

用法:
    uv run python scripts/prepare_alembic_takeover.py            # 只检查,不改
    uv run python scripts/prepare_alembic_takeover.py --apply    # 实际执行
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from sqlalchemy import inspect, text  # noqa: E402

from app.db import engine  # noqa: E402

# 这三张表由 0005_pools_and_new_columns 负责建立。
STRAY_TABLES = ("quant_pool_member", "quant_pool", "quant_trade_calendar")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Alembic 接管前校正生产库的混合状态")
    parser.add_argument("--apply", action="store_true",
                        help="实际执行(默认只检查)")
    args = parser.parse_args()

    inspector = inspect(engine)
    tables = set(inspector.get_table_names())

    if "alembic_version" in tables:
        print("alembic_version 已存在 —— Alembic 已接管,本脚本无需运行。")
        return

    to_drop: list[str] = []
    with engine.connect() as conn:
        for table in STRAY_TABLES:
            if table not in tables:
                print(f"  {table}: 不存在,跳过")
                continue
            rows = conn.execute(text(f"SELECT COUNT(*) FROM `{table}`")).scalar()
            if rows:
                print(f"❌ {table}: 有 {rows} 行数据,拒绝删除。"
                      "请人工确认这些数据是否需要保留后再处理。")
                sys.exit(1)
            print(f"  {table}: 空表(误建),待删除")
            to_drop.append(table)

    if not to_drop:
        print("\n无需校正,可直接 alembic upgrade。")
        return

    if not args.apply:
        print(f"\n[试运行] 将删除 {len(to_drop)} 张空表: {', '.join(to_drop)}")
        print("确认无误后加 --apply 执行。")
        return

    # 先子表后父表(quant_pool_member 有指向 quant_pool 的外键)
    with engine.begin() as conn:
        for table in to_drop:
            conn.exec_driver_sql(f"DROP TABLE `{table}`")
            print(f"已删除 {table}")

    print("\n✅ 校正完成。接下来按 logs/FINAL_REPORT.md 的顺序执行:")
    print("   1. mysqldump 备份")
    print("   2. uv run alembic upgrade 0005")
    print("   3. uv run python scripts/claim_legacy_user_data.py --user-id <admin UUID>")
    print("   4. uv run alembic upgrade head")


if __name__ == "__main__":
    main()
