"""本机:从指数成分文件重建 quant_index_member(不访问 baostock)。

读:
  data/baostock_raw/index/{hs300,zz500}/YYYY-MM-DD.csv.gz
  可选 manifest.json(记录下载参数)

写:
  quant_index_member(该指数全量替换)
  quant_stock(仅补缺失 code 的名称)

用法:
  uv run python scripts/ingest_index_members_from_files.py --estimate
  uv run python scripts/ingest_index_members_from_files.py
  uv run python scripts/ingest_index_members_from_files.py --indices hs300
"""
from __future__ import annotations

import argparse
import json
import logging
import sys
from datetime import date
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import pandas as pd  # noqa: E402

from app.data.universe import (  # noqa: E402
    INDEX_NAMES,
    rebuild_index_members_from_snapshots,
)
from app.db import SessionLocal  # noqa: E402

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    force=True,
)
logger = logging.getLogger("ingest_index_members")

ROOT = Path(__file__).resolve().parent.parent
RAW_ROOT = ROOT / "data" / "baostock_raw"
INDEX_ROOT = RAW_ROOT / "index"
MANIFEST_FILE = INDEX_ROOT / "manifest.json"


def _list_sample_files(index_name: str) -> list[tuple[date, Path]]:
    folder = INDEX_ROOT / index_name
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


def main() -> None:
    ap = argparse.ArgumentParser(description="从文件灌指数成分(不连 baostock)")
    ap.add_argument("--indices", default="hs300,zz500")
    ap.add_argument("--estimate", action="store_true")
    ap.add_argument(
        "--min-samples", type=int, default=10,
        help="采样点少于此数拒绝写入,防误灌空目录",
    )
    args = ap.parse_args()

    indices = [x.strip() for x in args.indices.split(",") if x.strip()]
    for name in indices:
        if name not in INDEX_NAMES:
            raise SystemExit(f"未知指数 {name}")

    if MANIFEST_FILE.exists():
        try:
            manifest = json.loads(MANIFEST_FILE.read_text())
            print("manifest:", json.dumps(manifest, ensure_ascii=False))
        except json.JSONDecodeError:
            print("manifest: (损坏,忽略)")

    plans: dict[str, list[tuple[date, Path]]] = {}
    for name in indices:
        files = _list_sample_files(name)
        plans[name] = files
        print(f"{name}: {len(files)} 个采样文件"
              f" [{files[0][0] if files else '-'} → {files[-1][0] if files else '-'}]")

    if args.estimate:
        return

    with SessionLocal() as db:
        for name in indices:
            files = plans[name]
            if len(files) < args.min_samples:
                raise SystemExit(
                    f"{name} 仅 {len(files)} 个文件,少于 --min-samples "
                    f"{args.min_samples};请先 download_index_members.py"
                )
            snapshots: list[tuple[date, dict[str, str]]] = []
            skipped_empty = 0
            for day, path in files:
                day, members = _load_snapshot(day, path)
                if not members:
                    skipped_empty += 1
                    logger.warning("空文件跳过 %s %s", name, day)
                    continue
                snapshots.append((day, members))
            if not snapshots:
                raise SystemExit(f"{name} 无有效采样,中止")
            result = rebuild_index_members_from_snapshots(db, name, snapshots)
            result["skipped_empty"] = skipped_empty
            print(name, result)

    print("完成(未调用 baostock;当前在册=最后采样日仍在册的代码)")


if __name__ == "__main__":
    main()
