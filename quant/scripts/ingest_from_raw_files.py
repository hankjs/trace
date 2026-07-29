"""本机:从 baostock_raw 文件批量灌库(不访问 baostock)。

读:
  data/baostock_raw/k/YYYY-MM-DD.csv.gz
  data/baostock_raw/factor/YYYY-MM-DD.csv.gz

写:
  quant_adjust_factor / quant_daily_bar / quant_valuation_snapshot

用法(开发机,连同一 MySQL):
  rsync -avz wananyun:/opt/hank-quant/data/baostock_raw/ data/baostock_raw/
  uv run python scripts/ingest_from_raw_files.py --estimate
  uv run python scripts/ingest_from_raw_files.py --sleep 0
"""
from __future__ import annotations

import argparse
import json
import logging
import sys
import time
from datetime import date
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import pandas as pd  # noqa: E402

from app.data import ingest  # noqa: E402
from app.db import SessionLocal  # noqa: E402

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    force=True,
)
logger = logging.getLogger("ingest_from_raw")

ROOT = Path(__file__).resolve().parent.parent
RAW_ROOT = Path(ROOT / "data" / "baostock_raw")
K_DIR = RAW_ROOT / "k"
FACTOR_DIR = RAW_ROOT / "factor"
STATE_FILE = RAW_ROOT / "ingest_state.json"


def _load_state() -> date | None:
    if not STATE_FILE.exists():
        return None
    try:
        raw = json.loads(STATE_FILE.read_text()).get("last_ingested_date")
        return date.fromisoformat(raw) if raw else None
    except (ValueError, json.JSONDecodeError):
        return None


def _save_state(day: date) -> None:
    STATE_FILE.write_text(json.dumps({
        "last_ingested_date": day.isoformat(),
        "updated_at": time.strftime("%Y-%m-%dT%H:%M:%S"),
    }))


def _days_with_files() -> list[date]:
    if not K_DIR.exists():
        return []
    days = []
    for p in sorted(K_DIR.glob("*.csv.gz")):
        try:
            days.append(date.fromisoformat(p.stem.replace(".csv", "")
                                           if p.stem.endswith(".csv")
                                           else p.name[:10]))
        except ValueError:
            # name is YYYY-MM-DD.csv.gz → stem YYYY-MM-DD.csv on some py, or YYYY-MM-DD
            name = p.name
            if name.endswith(".csv.gz"):
                days.append(date.fromisoformat(name[:-7]))
    return sorted(set(days))


def _read_k(day: date) -> pd.DataFrame:
    path = K_DIR / f"{day.isoformat()}.csv.gz"
    df = pd.read_csv(path, compression="gzip")
    if "date" in df.columns:
        df["date"] = pd.to_datetime(df["date"]).dt.date
    for col in ("open", "high", "low", "close", "volume", "amount",
                "pe_ttm", "pb", "ps_ttm"):
        if col in df.columns:
            df[col] = pd.to_numeric(df[col], errors="coerce")
    if "is_st" in df.columns:
        # csv 可能是 True/False/空
        def _st(v):
            if v is None or (isinstance(v, float) and pd.isna(v)):
                return None
            if isinstance(v, str) and v.strip() == "":
                return None
            if isinstance(v, str):
                return v.strip().lower() in ("1", "true", "t", "yes")
            return bool(v)
        df["is_st"] = df["is_st"].map(_st)
    return df


def _read_factor(day: date) -> pd.DataFrame:
    path = FACTOR_DIR / f"{day.isoformat()}.csv.gz"
    if not path.exists():
        return pd.DataFrame()
    df = pd.read_csv(path, compression="gzip")
    if df.empty:
        return df
    # 与 baostock_client 规范化列对齐
    rename = {
        "dividOperateDate": "divid_operate_date",
        "foreAdjustFactor": "fore_factor",
        "backAdjustFactor": "back_factor",
        "divid_operate_date": "divid_operate_date",
        "fore_factor": "fore_factor",
        "back_factor": "back_factor",
    }
    for a, b in rename.items():
        if a in df.columns and a != b:
            df = df.rename(columns={a: b})
    if "divid_operate_date" in df.columns:
        df["divid_operate_date"] = pd.to_datetime(
            df["divid_operate_date"], errors="coerce").dt.date
    for col in ("fore_factor", "back_factor"):
        if col in df.columns:
            df[col] = pd.to_numeric(df[col], errors="coerce")
    return df


def _ingest_one_day(db, day: date) -> dict:
    bars = _read_k(day)
    fac = _read_factor(day)
    result = {"day": str(day), "bars": 0, "factors": 0, "valuations": 0}

    # 1) 因子先入库(供前复权)
    if not fac.empty and "code" in fac.columns and "fore_factor" in fac.columns:
        for code, grp in fac.groupby("code"):
            result["factors"] += ingest.upsert_adjust_factors(db, code, grp)

    # 2) 用库内(含刚写入)因子换算前复权,整帧批量 upsert
    if bars.empty:
        return result
    codes = sorted(bars["code"].unique())
    factors = ingest._effective_fore_factors(db, codes, day)
    qfq_rows = []
    for r in bars.itertuples():
        if pd.isna(r.close):
            continue
        factor = factors.get(r.code, 1.0)
        qfq_rows.append({
            "code": r.code,
            "date": r.date if hasattr(r, "date") else day,
            "open": ingest.raw_to_qfq(r.open, factor),
            "high": ingest.raw_to_qfq(r.high, factor),
            "low": ingest.raw_to_qfq(r.low, factor),
            "close": ingest.raw_to_qfq(r.close, factor),
            "raw_close": float(r.close),
            "volume": 0.0 if pd.isna(getattr(r, "volume", 0)) else float(r.volume),
            "amount": 0.0 if pd.isna(getattr(r, "amount", 0)) else float(r.amount),
            "is_st": getattr(r, "is_st", None),
        })
    if qfq_rows:
        frame = pd.DataFrame(qfq_rows)
        # drop rows where qfq close failed
        frame = frame.dropna(subset=["close"])
        result["bars"] = ingest.upsert_bars_frame(db, frame)

    try:
        result["valuations"] = ingest.upsert_valuations_from_daily_bulk(db, day, bars)
    except Exception:  # noqa: BLE001
        db.rollback()
        logger.exception("估值落库失败 %s", day)
    return result


def main() -> int:
    global RAW_ROOT, K_DIR, FACTOR_DIR, STATE_FILE

    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--raw-root", default=str(RAW_ROOT))
    p.add_argument("--sleep", type=float, default=0.0)
    p.add_argument("--max-days", type=int, default=0)
    p.add_argument("--estimate", action="store_true")
    p.add_argument("--no-resume", action="store_true")
    args = p.parse_args()

    RAW_ROOT = Path(args.raw_root)
    K_DIR = RAW_ROOT / "k"
    FACTOR_DIR = RAW_ROOT / "factor"
    STATE_FILE = RAW_ROOT / "ingest_state.json"

    days = _days_with_files()
    if not args.no_resume:
        done = _load_state()
        if done:
            days = [d for d in days if d > done]

    if args.max_days > 0:
        days = days[: args.max_days]

    # 体积
    total_bytes = 0
    if K_DIR.exists():
        total_bytes = sum(f.stat().st_size for f in K_DIR.glob("*.csv.gz"))
        total_bytes += sum(f.stat().st_size for f in FACTOR_DIR.glob("*.csv.gz")) if FACTOR_DIR.exists() else 0

    print("======== 本机灌库试算(无 baostock) ========")
    print(f"RAW_ROOT: {RAW_ROOT}")
    print(f"磁盘已有 gzip: {total_bytes / 1024 / 1024:.1f} MB")
    print(f"待灌交易日: {len(days)}")
    if days:
        print(f"范围: {days[0]} ~ {days[-1]}")
    print("结论: OK" if days else "结论: 无文件可灌")
    if args.estimate or not days:
        return 0 if days or args.estimate else 1

    t0 = time.time()
    for i, day in enumerate(days, 1):
        with SessionLocal() as db:
            try:
                res = _ingest_one_day(db, day)
            except Exception:  # noqa: BLE001
                db.rollback()
                logger.exception("灌库失败 %s", day)
                return 1
        _save_state(day)
        if i % 5 == 0 or i == len(days):
            elapsed = time.time() - t0
            logger.info(
                "[%d/%d] %s bars=%s fac=%s val=%s 已用 %.0fs",
                i, len(days), day, res["bars"], res["factors"],
                res["valuations"], elapsed,
            )
        if args.sleep:
            time.sleep(args.sleep)
    logger.info("灌库完成 %d 日", len(days))
    # 离线灌库不经 scheduler,主动刷新旁路缓存,避免看板仍读旧 trust 结论
    try:
        from app.data.quality import refresh_data_quality_cache

        with SessionLocal() as db:
            report = refresh_data_quality_cache(db)
            s = report.get("summary") or {}
            logger.info(
                "data-quality 已刷新: alert=%s st_stock=%.1f%% latest_bar=%s",
                s.get("alert_level"),
                100 * float(s.get("st_stock_coverage_ratio") or 0),
                s.get("latest_bar_date"),
            )
    except Exception:  # noqa: BLE001
        logger.exception("data-quality 缓存刷新失败(源数据已灌完,可手动 force 刷新)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
