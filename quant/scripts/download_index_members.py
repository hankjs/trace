"""只下载指数历史成分到本地文件,不写业务表(串行,禁止并发)。

布局(与日 K 同属 baostock_raw,可一起 rsync):
  data/baostock_raw/index/{hs300,zz500}/YYYY-MM-DD.csv.gz
  列: code, name
  空结果也写 header-only 文件(resume 不再重打该日)
  data/baostock_raw/index/download_state.json
  data/baostock_raw/index/manifest.json  (start/end/step/indices)

默认 step=14: 2015-01-01→今 约 300 点/指数 × 2 ≈ 600 次 API。

用法:
  uv run python scripts/download_index_members.py --estimate --start 2015-01-01
  uv run python scripts/download_index_members.py --start 2015-01-01 --sleep 0.35

灌库见 scripts/ingest_index_members_from_files.py
"""
from __future__ import annotations

import argparse
import fcntl
import json
import logging
import os
import sys
import time
from datetime import date
from pathlib import Path

import pandas as pd

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.data import baostock_client  # noqa: E402
from app.data.clock import today_cst  # noqa: E402
from app.data.universe import INDEX_NAMES, sample_dates  # noqa: E402

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    force=True,
)
logger = logging.getLogger("download_index_members")

ROOT = Path(__file__).resolve().parent.parent
RAW_ROOT = Path(os.environ.get(
    "QUANT_BAOSTOCK_RAW", str(ROOT / "data" / "baostock_raw")))
INDEX_ROOT = RAW_ROOT / "index"
STATE_FILE = INDEX_ROOT / "download_state.json"
MANIFEST_FILE = INDEX_ROOT / "manifest.json"
LOCK_PATH = Path(os.environ.get(
    "QUANT_BAOSTOCK_LOCK", "/tmp/quant-baostock.lock"))


def _acquire_lock():
    LOCK_PATH.parent.mkdir(parents=True, exist_ok=True)
    fh = open(LOCK_PATH, "a+", encoding="utf-8")
    try:
        fcntl.flock(fh.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError as exc:
        fh.close()
        raise SystemExit(f"锁占用 {LOCK_PATH}(禁止与其它 baostock 任务并发)") from exc
    fh.seek(0)
    fh.truncate()
    fh.write(f"pid={os.getpid()} script=download_index_members\n")
    fh.flush()
    return fh


def _path(index_name: str, day: date) -> Path:
    return INDEX_ROOT / index_name / f"{day.isoformat()}.csv.gz"


def _write_frame(path: Path, df: pd.DataFrame) -> int:
    """原子写 gzip CSV。空 DataFrame 也落盘(header-only),作 empty 标记。"""
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    df.to_csv(tmp, index=False, compression="gzip")
    tmp.replace(path)
    return path.stat().st_size


def _write_empty_marker(path: Path) -> int:
    """空响应 durable 标记:resume 视为已完成,ingest 会跳过空成员。"""
    return _write_frame(path, pd.DataFrame(columns=["code", "name"]))


def _save_state(*, last_key: str, requests: int, bytes_written: int) -> None:
    INDEX_ROOT.mkdir(parents=True, exist_ok=True)
    prev: dict = {}
    if STATE_FILE.exists():
        try:
            prev = json.loads(STATE_FILE.read_text())
        except json.JSONDecodeError:
            prev = {}
    STATE_FILE.write_text(json.dumps({
        "last_key": last_key,
        "total_requests": int(prev.get("total_requests", 0)) + requests,
        "total_bytes": int(prev.get("total_bytes", 0)) + bytes_written,
        "updated_at": time.strftime("%Y-%m-%dT%H:%M:%S"),
    }, ensure_ascii=False, indent=2))


def _save_manifest(*, start: date, end: date, step_days: int,
                   indices: list[str]) -> None:
    INDEX_ROOT.mkdir(parents=True, exist_ok=True)
    MANIFEST_FILE.write_text(json.dumps({
        "start": start.isoformat(),
        "end": end.isoformat(),
        "step_days": step_days,
        "indices": indices,
        "updated_at": time.strftime("%Y-%m-%dT%H:%M:%S"),
    }, ensure_ascii=False, indent=2))


def _plan(start: date, end: date, step_days: int,
          indices: list[str]) -> list[tuple[str, date]]:
    days = sample_dates(start, end, step_days)
    return [(name, d) for name in indices for d in days]


def main() -> None:
    ap = argparse.ArgumentParser(description="下载指数历史成分到文件(不写库)")
    ap.add_argument("--start", default="2015-01-01")
    ap.add_argument("--end", default=None, help="默认今天(CST)")
    ap.add_argument("--step-days", type=int, default=14)
    ap.add_argument("--indices", default="hs300,zz500",
                    help="逗号分隔,默认 hs300,zz500")
    ap.add_argument("--sleep", type=float, default=0.35,
                    help="每次 baostock 请求后休眠秒数")
    ap.add_argument("--estimate", action="store_true",
                    help="只估算请求量与缺文件数,不登录")
    ap.add_argument("--force", action="store_true",
                    help="已有文件也重下")
    args = ap.parse_args()

    start = date.fromisoformat(args.start)
    end = date.fromisoformat(args.end) if args.end else today_cst()
    indices = [x.strip() for x in args.indices.split(",") if x.strip()]
    for name in indices:
        if name not in INDEX_NAMES:
            raise SystemExit(f"未知指数 {name},可选 {INDEX_NAMES}")

    jobs = _plan(start, end, args.step_days, indices)
    missing = [
        (n, d) for n, d in jobs
        if args.force or not _path(n, d).exists()
    ]
    print("======== 指数成分下载试算 ========")
    print(f"区间 {start} → {end}, step={args.step_days}")
    print(f"指数 {indices}")
    print(f"计划采样点(指数×日) {len(jobs)}")
    print(f"已有文件跳过 {len(jobs) - len(missing)}, 待下载 {len(missing)}")
    print(f"目录 {INDEX_ROOT}")
    if args.estimate:
        return

    if not missing:
        _save_manifest(start=start, end=end, step_days=args.step_days,
                       indices=indices)
        print("无需下载")
        return

    lock = _acquire_lock()
    ok = empty = fail = 0
    bytes_written = 0
    try:
        with baostock_client.login_session():
            for i, (name, day) in enumerate(missing, 1):
                path = _path(name, day)
                try:
                    df = baostock_client.fetch_index_members(name, day=day)
                except Exception:  # noqa: BLE001
                    logger.exception("下载失败 %s %s", name, day)
                    fail += 1
                    if args.sleep:
                        time.sleep(args.sleep)
                    continue
                written = 0
                if df is None or df.empty:
                    # 必须落盘:否则 resume 把 missing 当天反复请求,烧日配额
                    written = _write_empty_marker(path)
                    bytes_written += written
                    empty += 1
                    logger.warning(
                        "[%d/%d] 空结果写标记 %s %s bytes=%d",
                        i, len(missing), name, day, written,
                    )
                else:
                    # 统一列
                    out = df[["code", "name"]].copy() if "name" in df.columns \
                        else df[["code"]].assign(name="")
                    written = _write_frame(path, out)
                    bytes_written += written
                    ok += 1
                    logger.info(
                        "[%d/%d] %s %s rows=%d bytes=%d",
                        i, len(missing), name, day, len(out), written,
                    )
                _save_state(
                    last_key=f"{name}:{day.isoformat()}",
                    requests=1,
                    bytes_written=written,
                )
                if args.sleep:
                    time.sleep(args.sleep)
        _save_manifest(start=start, end=end, step_days=args.step_days,
                       indices=indices)
    finally:
        fcntl.flock(lock.fileno(), fcntl.LOCK_UN)
        lock.close()

    print(f"完成: ok={ok} empty={empty} fail={fail} bytes={bytes_written}")
    if fail:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
