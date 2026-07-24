"""spike 归档 + 回归基准:vectorbt 引擎对齐验证。

历史:一期手写引擎 vs vectorbt 复刻口径(T 日信号 T+1 开盘成交)误差 0.28%
(茅台近 1 年 ma_cross: -9.44% vs -9.72%,成交 15 笔一致),据此引擎切换到
vectorbt,手写 _backtest_one 已删除。

本脚本现在作为回归基准:新引擎跑同一口径,结果应与切换时的基准值吻合
(新引擎印花税改为按订单精确扣减,与旧基准允许 0.5% 内差异)。

运行: cd quant && uv run python scripts/spike_vbt.py
"""
from __future__ import annotations

import sys
from datetime import date, timedelta
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.backtest.engine import run_backtest
from app.db import SessionLocal

CODE = "sh.600519"
END = date.today()
START = END - timedelta(days=365)

# 切换时手写引擎基准(ma_cross 5/20,近 1 年)
# 2026-07 更新:印花税进入撮合 + 起点前已持仓首日合成建仓(本窗口起点仓位为 1,
# 多一次完整往返:15 -> 17 笔),原基准 -0.0944/15 记录的是旧口径的漏建仓结果
BASELINE = {"total_return": -0.1228, "trade_count": 17}


def main() -> None:
    with SessionLocal() as db:
        r = run_backtest(db, "ma_cross", [CODE], START, END, save=False)
    m = r["metrics"]["per_code"][CODE]
    print(f"新引擎: {m}")
    print(f"基准  : {BASELINE}")
    err = abs(m["total_return"] - BASELINE["total_return"])
    print(f"total_return 偏差: {err:.4f} ({err*100:.2f}%)")
    assert m["trade_count"] == BASELINE["trade_count"], "成交次数不一致"
    assert err < 0.005, "与基准偏差超过 0.5%"
    print("REGRESSION PASSED")


if __name__ == "__main__":
    main()
