"""未来函数校验:截断最后一天,策略输出的倒数第二个值必须不变。

单标的:positions(df[:-1])[-1] == positions(df)[-2]
组合:target_weights(dates[:-1])[-1] == target_weights(dates)[-2](逐列)

运行: cd quant && uv run python scripts/check_no_lookahead.py
"""
from __future__ import annotations

import sys
from datetime import date, timedelta
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import pandas as pd  # noqa: E402

from app.data.ingest import load_bars_df  # noqa: E402
from app.db import SessionLocal  # noqa: E402
from app.strategy.strategies import (PORTFOLIO_STRATEGIES, REGISTRY,  # noqa: E402
                                     SINGLE_STRATEGIES)

CODES = ["sh.600519", "sz.000001", "sh.600036"]
PARAM_SETS = [None, {"fast": 3, "slow": 15}, {"window": 15}, {"top_n": 5}]


def check_single(db) -> int:
    fails = 0
    for name in SINGLE_STRATEGIES:
        mod = REGISTRY[name]
        for code in CODES:
            df = load_bars_df(db, code, start=date.today() - timedelta(days=400))
            if len(df) < 90:
                print(f"SKIP {name} {code}: 数据不足 {len(df)}")
                continue
            for params in PARAM_SETS:
                full = mod.positions(df, params)
                trunc = mod.positions(df.iloc[:-1], params)
                if int(full.iat[-2]) != int(trunc.iat[-1]):
                    print(f"FAIL {name} {code} params={params}: "
                          f"full[-2]={full.iat[-2]} trunc[-1]={trunc.iat[-1]}")
                    fails += 1
    return fails


def check_portfolio(db) -> int:
    fails = 0
    pool_dfs = {}
    for code in CODES:
        df = load_bars_df(db, code, start=date.today() - timedelta(days=400))
        if len(df) >= 90:
            pool_dfs[code] = df
    dates = sorted({d for df in pool_dfs.values() for d in df["date"]})
    for name in PORTFOLIO_STRATEGIES:
        mod = REGISTRY[name]
        for params in PARAM_SETS:
            full = mod.target_weights(dates, pool_dfs, params)
            trunc = mod.target_weights(dates[:-1], pool_dfs, params)
            a = full.iloc[-2].fillna(0.0)
            b = trunc.iloc[-1].reindex(full.columns).fillna(0.0)
            if not (abs(a - b) < 1e-12).all():
                diff = (a - b).abs()
                print(f"FAIL {name} params={params}: max diff {diff.max():.6f}")
                fails += 1
    return fails


def main() -> None:
    with SessionLocal() as db:
        fails = check_single(db) + check_portfolio(db)
    if fails:
        print(f"\nLOOKAHEAD CHECK FAILED: {fails} 处")
        sys.exit(1)
    print("\nLOOKAHEAD CHECK PASSED")


if __name__ == "__main__":
    main()
