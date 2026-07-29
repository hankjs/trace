"""wananyun 侧:只下载 baostock 按日全市场数据到本地文件,不写业务表。

每个交易日 2 次 API:
  data/baostock_raw/k/YYYY-MM-DD.csv.gz
  data/baostock_raw/factor/YYYY-MM-DD.csv.gz  (可空文件=当日无因子行)

断点: data/baostock_raw/download_state.json

用法(服务器):
  cd /opt/hank-quant
  .venv/bin/python -u scripts/download_by_day.py --estimate --start 2015-01-01
  nohup .venv/bin/python -u scripts/download_by_day.py --start 2015-01-01 --sleep 0.3 \\
    >> /tmp/quant_download.log 2>&1 &

同步到开发机:
  rsync -avz --progress wananyun:/opt/hank-quant/data/baostock_raw/ quant/data/baostock_raw/
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

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import pandas as pd  # noqa: E402
from sqlalchemy import select  # noqa: E402

from app.data import baostock_client  # noqa: E402
from app.data import calendar as trade_calendar  # noqa: E402
from app.data.clock import today_cst  # noqa: E402
from app.db import SessionLocal  # noqa: E402
from app.models import TradeCalendar  # noqa: E402

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    force=True,
)
logger = logging.getLogger("download_by_day")

ROOT = Path(__file__).resolve().parent.parent
RAW_ROOT = Path(os.environ.get(
    "QUANT_BAOSTOCK_RAW", str(ROOT / "data" / "baostock_raw")))
K_DIR = RAW_ROOT / "k"
FACTOR_DIR = RAW_ROOT / "factor"
STATE_FILE = RAW_ROOT / "download_state.json"
LOCK_PATH = Path(os.environ.get("QUANT_BAOSTOCK_LOCK", "/tmp/quant-baostock.lock"))
REQ_PER_DAY = 2
DAILY_HARD = 50_000


def _acquire_lock():
    LOCK_PATH.parent.mkdir(parents=True, exist_ok=True)
    fh = open(LOCK_PATH, "a+", encoding="utf-8")
    try:
        fcntl.flock(fh.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError as exc:
        fh.close()
        raise SystemExit(f"锁占用 {LOCK_PATH}") from exc
    fh.seek(0)
    fh.truncate()
    fh.write(f"pid={os.getpid()} script=download_by_day\n")
    fh.flush()
    return fh


def _load_state() -> date | None:
    if not STATE_FILE.exists():
        return None
    try:
        raw = json.loads(STATE_FILE.read_text()).get("last_downloaded_date")
        return date.fromisoformat(raw) if raw else None
    except (ValueError, json.JSONDecodeError):
        return None


def _save_state(day: date, *, bytes_written: int = 0) -> None:
    STATE_FILE.parent.mkdir(parents=True, exist_ok=True)
    prev = {}
    if STATE_FILE.exists():
        try:
            prev = json.loads(STATE_FILE.read_text())
        except json.JSONDecodeError:
            prev = {}
    total = int(prev.get("total_bytes", 0)) + bytes_written
    STATE_FILE.write_text(json.dumps({
        "last_downloaded_date": day.isoformat(),
        "total_bytes": total,
        "updated_at": time.strftime("%Y-%m-%dT%H:%M:%S"),
    }))


def _k_path(day: date) -> Path:
    return K_DIR / f"{day.isoformat()}.csv.gz"


def _factor_path(day: date) -> Path:
    return FACTOR_DIR / f"{day.isoformat()}.csv.gz"


def _open_days(start: date, end: date) -> list[date]:
    with SessionLocal() as db:
        trade_calendar.sync_trade_calendar(db, start=start, end=end)
        return [r[0] for r in db.execute(
            select(TradeCalendar.date)
            .where(TradeCalendar.is_open.is_(True),
                   TradeCalendar.date >= start,
                   TradeCalendar.date <= end)
            .order_by(TradeCalendar.date)
        ).all()]


def _pending(days: list[date], *, resume: bool) -> list[date]:
    out = []
    done = _load_state() if resume else None
    for d in days:
        if done is not None and d <= done and _k_path(d).exists():
            continue
        if _k_path(d).exists() and _factor_path(d).exists():
            continue
        out.append(d)
    return out


def _write_df(path: Path, df: pd.DataFrame) -> int:
    path.parent.mkdir(parents=True, exist_ok=True)
    df.to_csv(path, index=False, compression="gzip")
    return path.stat().st_size


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--start", default="2015-01-01")
    p.add_argument("--end", default=None)
    p.add_argument("--sleep", type=float, default=0.3)
    p.add_argument("--max-days", type=int, default=0)
    p.add_argument("--max-requests", type=int, default=30_000)
    p.add_argument("--estimate", action="store_true")
    p.add_argument("--no-resume", action="store_true")
    p.add_argument("--no-lock", action="store_true")
    args = p.parse_args()

    start = date.fromisoformat(args.start)
    end = date.fromisoformat(args.end) if args.end else today_cst()
    days = _open_days(start, end)
    pending = _pending(days, resume=not args.no_resume)
    if args.max_days > 0:
        pending = pending[: args.max_days]

    req = len(pending) * REQ_PER_DAY
    # 体积粗估: 3KB/行 gzip 后约 0.3~0.5KB; 3000 行/日 → ~1~1.5MB/日
    est_mb = len(pending) * 1.2
    print("======== 按日下载(仅落盘)试算 ========")
    print(f"RAW_ROOT: {RAW_ROOT}")
    print(f"开市日总数: {len(days)}  待下载: {len(pending)}")
    print(f"API 乐观: {req}  (封顶 {args.max_requests})")
    print(f"磁盘粗估: ~{est_mb:.0f} MB gzip (全量约 2~4GB 量级)")
    if pending:
        print(f"范围: {pending[0]} ~ {pending[-1]}")
    if req > args.max_requests:
        print("结论: OVER")
        return 1
    if not pending:
        print("结论: 无待下载")
        return 0
    print("结论: OK")
    if args.estimate:
        return 0

    lock = None if args.no_lock else _acquire_lock()
    req_used = 0
    try:
        with baostock_client.login_session():
            for i, day in enumerate(pending, 1):
                if req_used + REQ_PER_DAY > args.max_requests:
                    logger.error("触及 max-requests=%s,停机", args.max_requests)
                    break
                n_bytes = 0
                if not _k_path(day).exists():
                    bars = baostock_client.fetch_market_daily_bars(day)
                    req_used += 1
                    n_bytes += _write_df(_k_path(day), bars)
                else:
                    logger.info("跳过已有 k %s", day)
                if not _factor_path(day).exists():
                    fac = baostock_client.fetch_market_adjust_factors(day)
                    req_used += 1
                    n_bytes += _write_df(_factor_path(day), fac)
                else:
                    logger.info("跳过已有 factor %s", day)
                _save_state(day, bytes_written=n_bytes)
                if i % 10 == 0 or i == len(pending):
                    logger.info(
                        "[%d/%d] %s done req≈%d bytes+≈%d",
                        i, len(pending), day, req_used, n_bytes,
                    )
                if args.sleep:
                    time.sleep(args.sleep)
        logger.info("下载结束 req≈%s state=%s", req_used, STATE_FILE)
        return 0
    finally:
        if lock is not None:
            try:
                fcntl.flock(lock.fileno(), fcntl.LOCK_UN)
            finally:
                lock.close()


if __name__ == "__main__":
    sys.exit(main())
