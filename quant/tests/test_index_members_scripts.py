"""指数成分两阶段 download/ingest 脚本契约。"""
from __future__ import annotations

from datetime import date
from pathlib import Path

import pandas as pd
from sqlalchemy import create_engine
from sqlalchemy.orm import Session

from app.data.universe import rebuild_index_members_from_snapshots
from app.db import Base
from app.models import IndexMember
from scripts.download_index_members import _write_empty_marker, _write_frame
from scripts.ingest_index_members_from_files import (
    _list_sample_files,
    load_valid_snapshots,
)


def _write_members(path: Path, members: dict[str, str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    df = pd.DataFrame(
        [{"code": c, "name": n} for c, n in members.items()],
        columns=["code", "name"],
    )
    _write_frame(path, df)


def test_write_empty_marker_is_resumable_and_skipped_on_ingest(tmp_path: Path):
    """empty 标记必须落盘(resume 跳过),且 ingest 不计为有效采样。"""
    day = date(2020, 1, 1)
    path = tmp_path / "hs300" / f"{day.isoformat()}.csv.gz"
    size = _write_empty_marker(path)
    assert path.exists()
    assert size > 0

    # resume 语义:文件存在即视为已完成
    assert path.exists()

    files = _list_sample_files("hs300", index_root=tmp_path)
    assert files == [(day, path)]
    snapshots, skipped = load_valid_snapshots(files)
    assert snapshots == []
    assert skipped == 1


def test_min_samples_counts_valid_snapshots_not_files(tmp_path: Path):
    """大量 empty 标记 + 少量有效文件时,有效采样数应远小于文件数。"""
    root = tmp_path / "hs300"
    # 11 empty + 1 valid
    for i in range(11):
        day = date(2020, 1, 1 + i)
        _write_empty_marker(root / f"{day.isoformat()}.csv.gz")
    valid_day = date(2020, 1, 20)
    _write_members(
        root / f"{valid_day.isoformat()}.csv.gz",
        {"sh.600000": "浦发"},
    )

    files = _list_sample_files("hs300", index_root=tmp_path)
    assert len(files) == 12
    snapshots, skipped = load_valid_snapshots(files)
    assert len(snapshots) == 1
    assert skipped == 11
    assert snapshots[0][0] == valid_day
    # 与 --min-samples 默认 10 对齐:12 文件但只有 1 有效 → 应拒绝
    assert len(snapshots) < 10


def test_ingest_guard_refuses_sparse_valid_then_accepts_enough(tmp_path: Path):
    """有效采样不足时不得 rebuild;够数后可写入。"""
    root = tmp_path / "hs300"
    # 3 个有效
    for i, code in enumerate(("sh.600000", "sh.600001", "sh.600519")):
        day = date(2020, 1, 1 + i * 14)
        _write_members(root / f"{day.isoformat()}.csv.gz", {code: f"n{i}"})
    # 再加 empty 充数
    for i in range(10):
        day = date(2019, 1, 1 + i)
        _write_empty_marker(root / f"{day.isoformat()}.csv.gz")

    files = _list_sample_files("hs300", index_root=tmp_path)
    snapshots, skipped = load_valid_snapshots(files)
    assert len(files) >= 10
    assert len(snapshots) == 3
    assert skipped == 10

    min_samples = 10
    assert len(snapshots) < min_samples  # 拒绝条件

    # 补到足够有效样本
    for i in range(7):
        day = date(2020, 3, 1 + i)
        _write_members(
            root / f"{day.isoformat()}.csv.gz",
            {"sh.600000": "浦发", f"sh.60{1000 + i}": f"x{i}"},
        )
    files = _list_sample_files("hs300", index_root=tmp_path)
    snapshots, _ = load_valid_snapshots(files)
    assert len(snapshots) >= min_samples

    engine = create_engine("sqlite+pysqlite:///:memory:")
    Base.metadata.create_all(engine)
    with Session(engine) as db:
        result = rebuild_index_members_from_snapshots(db, "hs300", snapshots)
        assert result["samples"] == len(snapshots)
        assert db.query(IndexMember).count() > 0
