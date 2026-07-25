#!/usr/bin/env python
"""校验「全新库 alembic upgrade head」与「models.py 的 create_all」产出的 schema 一致。

用 SQLAlchemy inspect() 逐表比对列(名/类型/可空)、主键、索引、唯一约束。
不一致则打印差异并以非零码退出 —— scripts/check_migrate_done.sh 依赖此行为。

**只在临时 sqlite 文件库上跑,不连任何 MySQL**(brief §2:生产库严格只读)。
"""
from __future__ import annotations

import subprocess
import sys
import tempfile
from pathlib import Path

from sqlalchemy import create_engine, inspect

QUANT_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(QUANT_DIR))

# alembic 自己维护的版本表,不参与比对
ALEMBIC_TABLES = {"alembic_version"}


def _build_via_alembic(db_path: Path) -> None:
    url = f"sqlite+pysqlite:///{db_path}"
    result = subprocess.run(
        [sys.executable, "-m", "alembic", "-x", f"db_url={url}", "upgrade", "head"],
        cwd=QUANT_DIR, capture_output=True, text=True,
    )
    if result.returncode != 0:
        print("alembic upgrade head 失败:")
        print(result.stdout)
        print(result.stderr)
        raise SystemExit(1)


def _build_via_models(db_path: Path) -> None:
    from app import models  # noqa: F401 - 注册模型
    from app.db import Base

    engine = create_engine(f"sqlite+pysqlite:///{db_path}")
    Base.metadata.create_all(engine)
    engine.dispose()


def _normalize_type(raw: object) -> str:
    """把方言类型对象归一成可比较字符串。

    sqlite 上 BigInteger 与 Integer 都渲染成 INTEGER,DECIMAL 渲染成 NUMERIC,
    两侧走同一条归一化,故差异只会来自真实的 schema drift。
    """
    text = str(raw).upper().replace(" ", "")
    if text in {"BIGINT", "INTEGER", "SMALLINT", "INT"}:
        return "INTEGER"
    if text.startswith("DECIMAL"):
        return "NUMERIC" + text[len("DECIMAL"):]
    if text in {"BOOLEAN", "BOOL"}:
        return "BOOLEAN"
    if text.startswith("VARCHAR"):
        return text
    return text


def _snapshot(db_path: Path) -> dict:
    engine = create_engine(f"sqlite+pysqlite:///{db_path}")
    inspector = inspect(engine)
    snapshot: dict = {}
    for table in inspector.get_table_names():
        if table in ALEMBIC_TABLES:
            continue
        columns = {
            col["name"]: (_normalize_type(col["type"]), bool(col["nullable"]))
            for col in inspector.get_columns(table)
        }
        pk = tuple(inspector.get_pk_constraint(table).get(
            "constrained_columns") or ())
        indexes = {
            (
                item["name"],
                tuple(item["column_names"]),
                bool(item.get("unique")),
            )
            for item in inspector.get_indexes(table)
        }
        uniques = {
            (item["name"], tuple(item["column_names"]))
            for item in inspector.get_unique_constraints(table)
        }
        snapshot[table] = {
            "columns": columns,
            "primary_key": pk,
            "indexes": indexes,
            "unique_constraints": uniques,
        }
    engine.dispose()
    return snapshot


def _diff(alembic_snap: dict, models_snap: dict) -> list[str]:
    problems: list[str] = []

    only_alembic = sorted(set(alembic_snap) - set(models_snap))
    only_models = sorted(set(models_snap) - set(alembic_snap))
    for table in only_alembic:
        problems.append(f"表 {table}: 迁移建了,models.py 里没有")
    for table in only_models:
        problems.append(f"表 {table}: models.py 里有,迁移没建")

    for table in sorted(set(alembic_snap) & set(models_snap)):
        a, m = alembic_snap[table], models_snap[table]

        for column in sorted(set(a["columns"]) - set(m["columns"])):
            problems.append(f"{table}.{column}: 迁移有此列,models.py 无")
        for column in sorted(set(m["columns"]) - set(a["columns"])):
            problems.append(f"{table}.{column}: models.py 有此列,迁移无")
        for column in sorted(set(a["columns"]) & set(m["columns"])):
            if a["columns"][column] != m["columns"][column]:
                problems.append(
                    f"{table}.{column}: 迁移 {a['columns'][column]} "
                    f"!= models {m['columns'][column]} (类型, 可空)"
                )

        if a["primary_key"] != m["primary_key"]:
            problems.append(
                f"{table} 主键: 迁移 {a['primary_key']} != models {m['primary_key']}")

        for key in ("indexes", "unique_constraints"):
            label = "索引" if key == "indexes" else "唯一约束"
            for item in sorted(a[key] - m[key]):
                problems.append(f"{table} {label} {item}: 迁移有,models.py 无")
            for item in sorted(m[key] - a[key]):
                problems.append(f"{table} {label} {item}: models.py 有,迁移无")

    return problems


def main() -> int:
    with tempfile.TemporaryDirectory() as tmp:
        alembic_db = Path(tmp) / "from_alembic.db"
        models_db = Path(tmp) / "from_models.db"
        _build_via_alembic(alembic_db)
        _build_via_models(models_db)
        problems = _diff(_snapshot(alembic_db), _snapshot(models_db))

    if problems:
        print(f"❌ 迁移产出与 models.py 不一致,共 {len(problems)} 处:")
        for item in problems:
            print(f"  - {item}")
        return 1
    print("✅ alembic upgrade head 与 models.py create_all 的 schema 一致")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
