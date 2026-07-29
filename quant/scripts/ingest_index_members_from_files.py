"""本机:从指数成分文件重建 quant_index_member。

读:
  data/baostock_raw/index/{hs300,zz500}/YYYY-MM-DD.csv.gz
  (可用环境变量 QUANT_BAOSTOCK_RAW 覆盖根目录,与 download 脚本一致)
  可选 manifest.json(记录下载参数)

写:
  quant_index_member(该指数全量替换)
  quant_stock(仅补缺失 code 的名称)

默认不访问 baostock。生产建议加 --live-sync,在历史区间写入后再对齐
当前成分(与 online rebuild_index_members(live_sync=True) 一致)。

用法:
  uv run python scripts/ingest_index_members_from_files.py --estimate
  uv run python scripts/ingest_index_members_from_files.py
  uv run python scripts/ingest_index_members_from_files.py --live-sync
  uv run python scripts/ingest_index_members_from_files.py --indices hs300
"""
from __future__ import annotations

import argparse
import json
import logging
import os
import sys
from datetime import date
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import pandas as pd  # noqa: E402

from app.data import baostock_client  # noqa: E402
from app.data.universe import (  # noqa: E402
    INDEX_NAMES,
    rebuild_index_members_from_snapshots,
    sync_index_members,
)
from app.db import SessionLocal  # noqa: E402

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    force=True,
)
logger = logging.getLogger("ingest_index_members")

ROOT = Path(__file__).resolve().parent.parent
RAW_ROOT = Path(os.environ.get(
    "QUANT_BAOSTOCK_RAW", str(ROOT / "data" / "baostock_raw")))
INDEX_ROOT = RAW_ROOT / "index"
MANIFEST_FILE = INDEX_ROOT / "manifest.json"


def _list_sample_files(index_name: str, index_root: Path | None = None) -> list[tuple[date, Path]]:
    folder = (index_root or INDEX_ROOT) / index_name
    if not folder.is_dir():
        return []
    out: list[tuple[date, Path]] = []
    for path in sorted(folder.glob("*.csv.gz")):
        name = path.name
        # YYYY-MM-DD.csv.gz
        if not name.endswith(".csv.gz"):
            continue
        try:
            day = date.fromisoformat(name[:-7])
        except ValueError:
            continue
        out.append((day, path))
    return out


def _load_snapshot(day: date, path: Path) -> tuple[date, dict[str, str]]:
    df = pd.read_csv(path, compression="gzip", dtype=str)
    if df.empty or "code" not in df.columns:
        return day, {}
    if "name" not in df.columns:
        df["name"] = ""
    members: dict[str, str] = {}
    for row in df.itertuples(index=False):
        code = str(getattr(row, "code", "") or "").strip()
        if not code:
            continue
        name = str(getattr(row, "name", "") or "").strip()
        members[code] = name
    return day, members


def load_valid_snapshots(
    files: list[tuple[date, Path]],
) -> tuple[list[tuple[date, dict[str, str]]], int]:
    """加载采样文件;跳过 empty/header-only 标记。返回 (有效快照, 跳过数)。"""
    snapshots: list[tuple[date, dict[str, str]]] = []
    skipped_empty = 0
    for day, path in files:
        day, members = _load_snapshot(day, path)
        if not members:
            skipped_empty += 1
            logger.warning("空文件跳过 %s", path)
            continue
        snapshots.append((day, members))
    return snapshots, skipped_empty


def main() -> None:
    ap = argparse.ArgumentParser(
        description="从文件灌指数成分(默认不连 baostock;可用 --live-sync)",
    )
    ap.add_argument("--indices", default="hs300,zz500")
    ap.add_argument("--estimate", action="store_true")
    ap.add_argument(
        "--min-samples", type=int, default=10,
        help="有效(非空)采样点少于此数拒绝写入,防误灌空/稀疏目录",
    )
    ap.add_argument(
        "--live-sync", action="store_true",
        help="写入后调 baostock 当前成分增量同步(对齐 out_date is NULL)",
    )
    args = ap.parse_args()

    indices = [x.strip() for x in args.indices.split(",") if x.strip()]
    for name in indices:
        if name not in INDEX_NAMES:
            raise SystemExit(f"未知指数 {name}")

    print(f"目录 {INDEX_ROOT}")
    if MANIFEST_FILE.exists():
        try:
            manifest = json.loads(MANIFEST_FILE.read_text())
            print("manifest:", json.dumps(manifest, ensure_ascii=False))
        except json.JSONDecodeError:
            print("manifest: (损坏,忽略)")

    # name -> (files, snapshots, skipped_empty)
    plans: dict[str, tuple[
        list[tuple[date, Path]],
        list[tuple[date, dict[str, str]]],
        int,
    ]] = {}
    for name in indices:
        files = _list_sample_files(name)
        # estimate 也统计有效样本,避免只看文件数被 empty 标记误导
        snapshots, skipped_empty = load_valid_snapshots(files)
        plans[name] = (files, snapshots, skipped_empty)
        print(
            f"{name}: 文件 {len(files)}, 有效 {len(snapshots)}, "
            f"空标记/坏文件 {skipped_empty}"
            f" [{files[0][0] if files else '-'} → {files[-1][0] if files else '-'}]"
        )

    if args.estimate:
        return

    with SessionLocal() as db:
        for name in indices:
            files, snapshots, skipped_empty = plans[name]
            if len(snapshots) < args.min_samples:
                raise SystemExit(
                    f"{name} 仅 {len(snapshots)} 个有效采样"
                    f"(文件 {len(files)}, 空/坏 {skipped_empty}),"
                    f"少于 --min-samples {args.min_samples};"
                    f"请先 download_index_members.py 或检查 empty 标记占比"
                )
            result = rebuild_index_members_from_snapshots(db, name, snapshots)
            result["skipped_empty"] = skipped_empty
            result["files"] = len(files)
            print(name, result)

        if args.live_sync:
            print("live-sync: 拉取 baostock 当前成分对齐 out_date is NULL ...")
            with baostock_client.login_session():
                for name in indices:
                    sync_result = sync_index_members(db, name)
                    print(f"sync {name}", sync_result)
        else:
            print(
                "完成(未 live-sync)。当前在册=最后有效采样日仍在册的代码;"
                "生产请追加 --live-sync 或稍后 /api/admin 同步成分,"
                "以免调仓后 current_pool 滞后。",
            )


if __name__ == "__main__":
    main()
