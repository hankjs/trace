"""只补 quant_daily_bar.is_st 一列,不重拉价格。

## 为什么需要专用脚本

`is_st` 是 alembic 0010 新增的列,既有行为 NULL。用 `backfill_pool.py
--force-rescale all` 补它是错的做法。这里只 UPDATE is_st,不碰价格列。

## baostock 硬约束

- 每日 API ≤ 5 万次;默认每只约 **1** 次请求(失败按年分块会变多)。
- **禁止并发**; flock + 封禁错误码立即停机。
- 详见 DATA-ARCHITECTURE.md §5 / docs/baostock-bulk-ingest.md。

## 断点 / 不重复拉取

- **跳过已完整**的 code:该股所有日线行 is_st 均非 NULL → 不请求 baostock。
- **只处理有缺口**的 code:存在 is_st IS NULL 的 bar。
- **UPDATE 只写空位**:`WHERE is_st IS NULL`,已有值不覆盖。
- 区间默认收窄到该股「缺口 bar 的 min~max 日期」,减少无效历史传输。

## 用法

    # 1) 只读试算(不连 baostock、不占锁)
    uv run python scripts/backfill_is_st.py --estimate

    # 2) 确认未超日配额后再跑
    uv run python scripts/backfill_is_st.py --sleep 1.2 --max-requests 40000
"""
from __future__ import annotations

import argparse
import fcntl
import logging
import os
import socket
import sys
import time
from dataclasses import dataclass
from datetime import date
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import baostock as bs  # noqa: E402
from sqlalchemy import text  # noqa: E402

from app.data import baostock_client, ingest  # noqa: E402
from app.data.clock import today_cst  # noqa: E402
from app.db import SessionLocal  # noqa: E402

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    force=True,
)
logger = logging.getLogger("backfill_is_st")

DEFAULT_SLEEP_PER_CODE = 0.5
DEFAULT_SLEEP_PER_CHUNK = 0.3
DEFAULT_MAX_REQUESTS = 40_000
# 官方硬上限;试算与本轮预算都对照这个
DAILY_API_HARD_LIMIT = 50_000
# 给盘后调度等预留
DEFAULT_DAILY_RESERVE = 5_000
RETRY = 3
SOCKET_TIMEOUT_SECONDS = 60
LOCK_PATH = Path(os.environ.get(
    "QUANT_BAOSTOCK_LOCK", "/tmp/quant-baostock.lock"))

BAN_MARKERS = (
    "10001011",
    "黑名单",
    "blacklist",
    "用户状态不正常",
    "超过访问频率",
    "访问频次",
)


class BaostockBannedError(RuntimeError):
    """出口 IP 已被限速/拉黑,必须停机。"""


@dataclass(frozen=True)
class PendingJob:
    code: str
    null_bars: int
    total_bars: int
    gap_start: date
    gap_end: date


def _looks_banned(msg: str) -> bool:
    low = msg.lower()
    return any(m.lower() in low for m in BAN_MARKERS)


def _acquire_lock() -> object:
    LOCK_PATH.parent.mkdir(parents=True, exist_ok=True)
    fh = open(LOCK_PATH, "a+", encoding="utf-8")
    try:
        fcntl.flock(fh.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError as exc:
        fh.close()
        raise SystemExit(
            f"已有 baostock 任务占用锁 {LOCK_PATH};禁止并发连接。"
        ) from exc
    fh.seek(0)
    fh.truncate()
    fh.write(f"pid={os.getpid()} script=backfill_is_st\n")
    fh.flush()
    return fh


def list_pending_jobs(
    db,
    *,
    codes_filter: list[str] | None = None,
    global_start: date,
    global_end: date,
) -> tuple[list[PendingJob], dict[str, int]]:
    """找出 is_st 仍有缺口的非北交所股票;已完整的不进列表。

    返回 (jobs, stats)。
    """
    # 按 code 聚合:只保留「有 bar 且存在 is_st 为空」的股票
    # gap 区间 = 该股 NULL is_st 行的 min/max date,再与全局 [start,end] 求交
    sql = text("""
        SELECT
            code,
            COUNT(*) AS total_bars,
            COUNT(is_st) AS known_bars,
            MIN(CASE WHEN is_st IS NULL THEN date END) AS gap_start,
            MAX(CASE WHEN is_st IS NULL THEN date END) AS gap_end
        FROM quant_daily_bar
        WHERE code NOT LIKE :bj
        GROUP BY code
        HAVING COUNT(is_st) < COUNT(*)
        ORDER BY code
    """)
    rows = db.execute(sql, {"bj": f"{ingest.BJ_PREFIX}%"}).all()

    complete_like = db.execute(text("""
        SELECT COUNT(*) FROM (
            SELECT code FROM quant_daily_bar
            WHERE code NOT LIKE :bj
            GROUP BY code
            HAVING COUNT(*) > 0 AND COUNT(is_st) = COUNT(*)
        ) t
    """), {"bj": f"{ingest.BJ_PREFIX}%"}).scalar()

    filter_set = set(codes_filter) if codes_filter else None
    jobs: list[PendingJob] = []
    skipped_filter = 0
    for r in rows:
        code = r.code
        if filter_set is not None and code not in filter_set:
            skipped_filter += 1
            continue
        if r.gap_start is None or r.gap_end is None:
            continue
        gap_start = max(r.gap_start, global_start)
        gap_end = min(r.gap_end, global_end)
        if gap_start > gap_end:
            continue
        null_bars = int(r.total_bars) - int(r.known_bars)
        jobs.append(PendingJob(
            code=code,
            null_bars=null_bars,
            total_bars=int(r.total_bars),
            gap_start=gap_start,
            gap_end=gap_end,
        ))

    stats = {
        "complete_codes": int(complete_like or 0),
        "incomplete_codes": len(rows),
        "jobs": len(jobs),
        "skipped_by_code_filter": skipped_filter,
        "null_bars_total": sum(j.null_bars for j in jobs),
    }
    return jobs, stats


def estimate_requests(jobs: list[PendingJob]) -> dict[str, float | int]:
    """试算请求量(乐观=每只 1 次;悲观=按年分块)。"""
    n = len(jobs)
    # 乐观:整段一次成功
    optimistic = n
    # 悲观:每只按 gap 跨年数分块(失败回退路径)
    pessimistic = 0
    for j in jobs:
        years = j.gap_end.year - j.gap_start.year + 1
        pessimistic += max(1, years)
    return {
        "codes": n,
        "req_optimistic": optimistic,
        "req_pessimistic": pessimistic,
        "req_planning": optimistic,  # 正常链路按乐观估;上限对照悲观
    }


def print_estimate(
    jobs: list[PendingJob],
    stats: dict[str, int],
    *,
    sleep_s: float,
    max_requests: int,
    max_codes: int,
    daily_limit: int = DAILY_API_HARD_LIMIT,
    daily_reserve: int = DEFAULT_DAILY_RESERVE,
) -> int:
    """打印试算。返回 0=可跑, 1=超限建议减量, 2=无任务。"""
    est = estimate_requests(jobs)
    if max_codes > 0:
        jobs = jobs[:max_codes]
        est = estimate_requests(jobs)

    budget = max(0, daily_limit - daily_reserve)
    effective_cap = min(max_requests, budget)
    opt = int(est["req_optimistic"])
    pes = int(est["req_pessimistic"])
    # 墙钟:每只 sleep + 约 1s 查询
    eta_sec = len(jobs) * (sleep_s + 1.0)

    print("======== is_st 回补试算(不连 baostock) ========")
    print(f"已完整跳过 codes:     {stats['complete_codes']}")
    print(f"库内有缺口 codes:     {stats['incomplete_codes']}")
    print(f"本轮将请求 codes:     {len(jobs)}")
    print(f"缺口 bar 合计:        {sum(j.null_bars for j in jobs)}")
    print(f"API 请求(乐观/每只1): {opt}")
    print(f"API 请求(悲观/按年):  {pes}")
    print(f"官方日上限:           {daily_limit}")
    print(f"预留调度额度:         {daily_reserve}")
    print(f"本轮可用预算:         {budget} (limit-reserve)")
    print(f"--max-requests:       {max_requests}")
    print(f"实际封顶:             {effective_cap}")
    print(f"sleep/只:             {sleep_s}s")
    print(f"预计耗时:             {eta_sec / 3600:.2f} h "
          f"(~{int(eta_sec)}s, 按 sleep+1s/只)")
    if jobs:
        print(f"样例前 5 只:          "
              f"{', '.join(f'{j.code}[{j.gap_start}~{j.gap_end} null={j.null_bars}]' for j in jobs[:5])}")

    ok_opt = opt <= effective_cap
    ok_pes = pes <= effective_cap
    if len(jobs) == 0:
        print("结论: 无待补任务,无需请求 baostock。")
        return 2
    if ok_opt and ok_pes:
        print(f"结论: OK — 乐观/悲观均 ≤ {effective_cap},可开跑。")
        return 0
    if ok_opt and not ok_pes:
        print(
            f"结论: CAUTION — 乐观 {opt} ≤ 预算,但悲观分块 {pes} 可能顶满;"
            f"建议保持 sleep≥1s,失败分块会多耗配额。"
        )
        return 0
    # 超限:给出建议 max-codes
    suggest = effective_cap  # 乐观下每只 1 请求
    print(
        f"结论: OVER — 乐观请求 {opt} > 实际封顶 {effective_cap}。"
        f"请加 --max-codes {suggest} 分多日跑,或提高 sleep 拖慢但日配额仍超则必须拆天。"
    )
    return 1


def _fetch_is_st_range(code: str, start: date, end: date) -> dict[date, bool]:
    rs = bs.query_history_k_data_plus(
        code, "date,isST", start_date=start.isoformat(),
        end_date=end.isoformat(), frequency="d", adjustflag="2",
    )
    err = f"{rs.error_code} {rs.error_msg}"
    if rs.error_code != "0":
        if _looks_banned(err):
            raise BaostockBannedError(f"{code}: {err}")
        raise RuntimeError(f"{code}: {err}")
    out: dict[date, bool] = {}
    while (rs.error_code == "0") & rs.next():
        day_s, st = rs.get_row_data()
        if not day_s or st not in ("0", "1"):
            continue
        out[date.fromisoformat(day_s)] = st == "1"
    if rs.error_code != "0" and _looks_banned(f"{rs.error_code} {rs.error_msg}"):
        raise BaostockBannedError(
            f"{code} 读结果中: {rs.error_code} {rs.error_msg}")
    return out


def fetch_is_st(
    code: str,
    start: date,
    end: date,
    *,
    sleep_chunk: float,
    request_counter: list[int],
) -> dict[date, bool]:
    try:
        request_counter[0] += 1
        return _fetch_is_st_range(code, start, end)
    except BaostockBannedError:
        raise
    except Exception:  # noqa: BLE001
        out: dict[date, bool] = {}
        for year in range(start.year, end.year + 1):
            chunk_start = max(start, date(year, 1, 1))
            chunk_end = min(end, date(year, 12, 31))
            request_counter[0] += 1
            out.update(_fetch_is_st_range(code, chunk_start, chunk_end))
            time.sleep(sleep_chunk)
        return out


def apply_is_st(db, code: str, values: dict[date, bool]) -> int:
    """只更新 is_st 仍为 NULL 的行,不覆盖已有值。"""
    if not values:
        return 0
    rows = [{"c": code, "d": day, "v": st} for day, st in values.items()]
    # executemany: 带 is_st IS NULL 条件
    result = db.execute(
        text(
            "UPDATE quant_daily_bar SET is_st = :v "
            "WHERE code = :c AND date = :d AND is_st IS NULL"
        ),
        rows,
    )
    db.commit()
    # rowcount 在批量时因方言可能不准;用 len 作上界日志即可
    try:
        return int(result.rowcount or 0)
    except Exception:  # noqa: BLE001
        return len(rows)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="只补 quant_daily_bar.is_st(跳过已完整,试算后再跑)")
    parser.add_argument("--start", default="2015-01-01",
                        help="全局最早日期(与缺口区间求交)")
    parser.add_argument("--end", default="",
                        help="全局最晚日期,默认今天(上海)")
    parser.add_argument("--codes", default="", help="逗号分隔白名单;仍会跳过已完整")
    parser.add_argument("--sleep", type=float, default=DEFAULT_SLEEP_PER_CODE)
    parser.add_argument("--sleep-chunk", type=float, default=DEFAULT_SLEEP_PER_CHUNK)
    parser.add_argument("--max-codes", type=int, default=0,
                        help="本轮最多处理只数;0=不限制")
    parser.add_argument("--max-requests", type=int, default=DEFAULT_MAX_REQUESTS)
    parser.add_argument("--daily-reserve", type=int, default=DEFAULT_DAILY_RESERVE,
                        help="为盘后调度预留的日配额")
    parser.add_argument("--estimate", action="store_true",
                        help="只试算请求量与是否超限,不连 baostock、不占锁")
    parser.add_argument("--dry-run", action="store_true",
                        help="同 --estimate(兼容旧参数)")
    parser.add_argument("--force-run", action="store_true",
                        help="试算 OVER 时仍强制开跑(不推荐)")
    parser.add_argument("--no-lock", action="store_true")
    args = parser.parse_args()

    estimate_only = args.estimate or args.dry_run
    global_start = date.fromisoformat(args.start)
    global_end = date.fromisoformat(args.end) if args.end else today_cst()
    codes_filter = (
        [c.strip() for c in args.codes.split(",") if c.strip()]
        if args.codes else None
    )

    with SessionLocal() as db:
        jobs, stats = list_pending_jobs(
            db,
            codes_filter=codes_filter,
            global_start=global_start,
            global_end=global_end,
        )

    if estimate_only:
        rc = print_estimate(
            jobs, stats,
            sleep_s=args.sleep,
            max_requests=args.max_requests,
            max_codes=args.max_codes,
            daily_reserve=args.daily_reserve,
        )
        raise SystemExit(rc)

    # 正式跑前内嵌试算;超限且无 --force-run 则拒绝
    rc = print_estimate(
        jobs, stats,
        sleep_s=args.sleep,
        max_requests=args.max_requests,
        max_codes=args.max_codes,
        daily_reserve=args.daily_reserve,
    )
    if rc == 2:
        raise SystemExit(0)
    if rc == 1 and not args.force_run:
        logger.error("试算超限,拒绝开跑。请 --estimate 后用 --max-codes 拆天,"
                     "或显式 --force-run(不推荐)。")
        raise SystemExit(1)

    if args.max_codes > 0:
        jobs = jobs[: args.max_codes]

    lock_fh = None if args.no_lock else _acquire_lock()
    updated = failed = 0
    failed_codes: list[str] = []
    request_counter = [0]
    try:
        socket.setdefaulttimeout(SOCKET_TIMEOUT_SECONDS)
        t0 = time.time()
        with SessionLocal() as db, baostock_client.login_session():
            for i, job in enumerate(jobs, 1):
                if request_counter[0] >= args.max_requests:
                    logger.error(
                        "已达 --max-requests=%d,安全停机;剩余 %d 只下次续跑",
                        args.max_requests, len(jobs) - i + 1,
                    )
                    break
                for attempt in range(1, RETRY + 1):
                    try:
                        values = fetch_is_st(
                            job.code, job.gap_start, job.gap_end,
                            sleep_chunk=args.sleep_chunk,
                            request_counter=request_counter,
                        )
                        n = apply_is_st(db, job.code, values)
                        updated += n
                        break
                    except BaostockBannedError as exc:
                        db.rollback()
                        logger.error("封禁/限速,立即停机: %s", exc)
                        raise SystemExit(2) from exc
                    except Exception as exc:  # noqa: BLE001
                        db.rollback()
                        if _looks_banned(repr(exc)):
                            logger.error("异常含封禁特征,停机: %r", exc)
                            raise SystemExit(2) from exc
                        if attempt == RETRY:
                            logger.warning("失败 %s: %r", job.code, exc)
                            failed += 1
                            failed_codes.append(job.code)
                        else:
                            time.sleep(2 * attempt)
                if i % 20 == 0 or i == len(jobs):
                    elapsed = time.time() - t0
                    remain = elapsed / i * (len(jobs) - i) if i else 0
                    logger.info(
                        "进度 %d/%d req≈%d 已用 %.0fs 剩余≈%.0fs "
                        "写入(约)%d 失败 %d",
                        i, len(jobs), request_counter[0], elapsed, remain,
                        updated, failed,
                    )
                time.sleep(args.sleep)

        logger.info(
            "结束: 约更新 %d 行,失败 %d 只,API≈%d;失败样例 %s",
            updated, failed, request_counter[0], failed_codes[:20],
        )
    finally:
        if lock_fh is not None:
            try:
                fcntl.flock(lock_fh.fileno(), fcntl.LOCK_UN)
            finally:
                lock_fh.close()


if __name__ == "__main__":
    main()
