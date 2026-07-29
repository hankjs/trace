"""把书库 P1 候选配置为 admin 名下的 StrategySpec 并入库。

背景:docs/research/strategy-candidates.md 已把 16 本书的候选去重为 canonical
注册表,其中 P1 类不依赖点时财务/行业数据,用现有日线字段与操作符即可表达。
本脚本复刻 POST /api/strategies 的创建路径(能力校验 -> parse -> with_status
-> hash -> insert),只是把 owner 固定为 admin。按 (owner_id, name) 幂等,
已存在则跳过。

不配置的 P1 项及原因:
- CAN-REG-01 市场状态门控:需要指数/宽度字段,SUPPORTED_FIELDS 没有(missing_data)
- CAN-VOL-01 波动率预测:是风险估计组件,不产出仓位信号
- CAN-VOL-02 组合波动缩放:positioning 无目标波动缩放语义(missing_engine)
- CAN-MON-01 成本缓冲调仓:RebalanceSpec 无缓冲参数(missing_engine)
- CAN-EXIT-02/04、CAN-PORT-01:是离场/约束部件,已体现在各 spec 的
  native_exit / overlays / portfolio_constraints 中;CAN-EXIT-04 单独成一条
  消融变体(第 9 条)。

用法:uv run python scripts/seed_p1_strategies.py
"""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from sqlalchemy import text  # noqa: E402

from app.data.ingest import BAR_FIELDS, snapshot_available_fields  # noqa: E402
from app.db import SessionLocal  # noqa: E402
from app.models import Strategy  # noqa: E402
from app.strategy.evidence import with_status  # noqa: E402
from app.strategy.spec import (  # noqa: E402
    CapabilityStatus,
    parse_strategy_spec,
    resolve_capabilities,
    strategy_spec_hash,
)

ADMIN_USERNAME = "admin"

# ---------------------------------------------------------------- 表达式助手

def field(name):
    return {"op": "field", "name": name}


def lit(value):
    return {"op": "literal", "value": value}


def binary(op, left, right):
    return {"op": op, "left": left, "right": right}


def rolling(op, source, window, shift):
    return {"op": op, "input": source, "window": window, "shift": shift}


def indicator(op, source, window):
    return {"op": op, "input": source, "window": window}


def all_of(*args):
    return {"op": "all", "args": list(args)}


def any_of(*args):
    return {"op": "any", "args": list(args)}


CLOSE, HIGH, LOW, VOLUME = field("close"), field("high"), field("low"), field("volume")

# ---------------------------------------------------------------- 公共部件

EXECUTION = {
    "buy_limit_policy": "reject",
    "cost_model": "a_share_daily_v1",
    "execution_time": "next_open",
    "max_entry_premium": 0.0,
    "missing_bar_policy": "reject_entry_retry_exit",
    "sell_limit_policy": "retry",
    "signal_time": "close",
    "suspension_policy": "reject_entry_retry_exit",
}

HOLDING = {
    "allow_add": False,
    "allow_reduce": False,
    "cooldown_days": 0,
    "risk_reentry": "native_reset",
}

UNIVERSE = {
    "exclude_st": True,
    "min_amount_avg20": 0.0,
    "min_listing_days": 60,
    "pool_id": 2,  # 全部A股
}

VALIDATION = {
    "baseline_ids": ["buy_and_hold", "equal_weight"],
    "locked_oos": True,
    "parameter_scans": [],
    "rejection_criteria": [
        "no_net_oos_increment", "unstable_parameters", "capacity_failure",
    ],
}


def overlays(risk=None):
    default = {
        "enabled": False, "type": "fixed_pct", "value": 0.08,
        "atr_period": 14, "trailing": False,
    }
    return {
        "risk": {**default, **(risk or {})},
        "take_profit": {
            "enabled": False, "type": "fixed_pct", "value": 0.2,
            "atr_period": 14, "trailing": False,
        },
    }


def constraints(max_positions=500):
    return {
        "long_only": True,
        "max_positions": max_positions,
        "max_single_weight": 1.0,
        "max_total_weight": 1.0,
    }


def requirements(*fields):
    return [
        {"availability": "daily_close", "field": name, "required": True}
        for name in fields
    ]


def single_spec(*, canonical_id, hypothesis, sources, entry, entry_reason,
                exit_, exit_reason, fields, risk_overlay=None):
    return {
        "schema_version": 1,
        "kind": "single",
        "metadata": {
            "canonical_id": canonical_id,
            "evidence_status": "unverified",
            "hypothesis": hypothesis,
            "sources": sources,
        },
        "universe": UNIVERSE,
        "data_requirements": requirements(*fields),
        "entry": {"condition": entry, "reason_code": entry_reason},
        "native_exit": {"condition": exit_, "reason_code": exit_reason},
        "positioning": {"target": 1.0, "type": "binary"},
        "holding": HOLDING,
        "overlays": overlays(risk_overlay),
        "portfolio_constraints": constraints(),
        "execution": EXECUTION,
        "validation": VALIDATION,
    }


def src(book, candidate_id):
    return {"book": book, "candidate_id": candidate_id}


CHANNEL_EXIT = binary("lt", CLOSE, rolling("rolling_min", LOW, 10, 1))

# ---------------------------------------------------------------- P1 候选

SPECS = [
    # 1. CAN-TRD-01 移动平均趋势基线
    ("P1 双均线趋势基线", single_spec(
        canonical_id="CAN-TRD-01",
        hypothesis="短期均价高于长期均价时趋势可能延续，均线关系 reversal 作为假设失效。",
        sources=[src("股市趋势技术分析", "TREND-08"),
                 src("Evidence-Based Technical Analysis", "EBTA-RULE-01")],
        entry=binary("gt", indicator("ma", CLOSE, 5), indicator("ma", CLOSE, 20)),
        entry_reason="fast_ma_above_slow",
        exit_=binary("lte", indicator("ma", CLOSE, 5), indicator("ma", CLOSE, 20)),
        exit_reason="fast_ma_not_above_slow",
        fields=("close",),
    )),
    # 2. CAN-TRD-02 区间突破与相对放量
    ("P1 突破与相对放量", single_spec(
        canonical_id="CAN-TRD-02",
        hypothesis="收盘突破过去 20 日最高价且相对放量时，突破延续概率可能高于无量突破。",
        sources=[src("股市趋势技术分析", "TREND-03"),
                 src("简派交易", "SIMPLE-03"),
                 src("股票大作手回忆录", "LIV-01")],
        entry=all_of(
            binary("gt", CLOSE, rolling("rolling_max", HIGH, 20, 1)),
            binary("gt", rolling("volume_ratio", VOLUME, 20, 1), lit(1.5)),
        ),
        entry_reason="breakout_with_volume",
        exit_=CHANNEL_EXIT,
        exit_reason="close_below_prior_low",
        fields=("close", "high", "low", "volume"),
    )),
    # 3. CAN-TRD-03 突破回踩与确认
    ("P1 突破回踩确认", single_spec(
        canonical_id="CAN-TRD-03",
        hypothesis="突破后回踩突破位附近(±容差)并重新站上短均线，可能比立即追突破有更好的赔率。",
        sources=[src("股市趋势技术分析", "TREND-04"),
                 src("简派交易", "SIMPLE-04"),
                 src("日本蜡烛图技术", "CANDLE-06")],
        entry=all_of(
            # 近 5 日内曾收盘突破 20 日高点
            binary("gt", rolling("rolling_max", CLOSE, 5, 1),
                   rolling("rolling_max", HIGH, 20, 6)),
            # 回踩到突破位 +3% 以内
            binary("lte", LOW,
                   binary("multiply", rolling("rolling_max", HIGH, 20, 6), lit(1.03))),
            # 未显著跌破突破位(-10% 容差)
            binary("gte", LOW,
                   binary("multiply", rolling("rolling_max", HIGH, 20, 6), lit(0.90))),
            # 恢复确认:收盘重新站上 5 日均线
            binary("gt", CLOSE, indicator("ma", CLOSE, 5)),
        ),
        entry_reason="breakout_pullback_resume",
        exit_=CHANNEL_EXIT,
        exit_reason="close_below_prior_low",
        fields=("close", "high", "low"),
    )),
    # 4. CAN-TRD-04 波动/成交收缩后的放量突破
    ("P1 缩量后放量突破", single_spec(
        canonical_id="CAN-TRD-04",
        hypothesis="价格和成交收缩后的放量向上突破可能形成趋势，平台下沿作为失效线。",
        sources=[src("股市趋势技术分析", "TREND-05"),
                 src("量化交易从入门到精通", "QTP-002"),
                 src("笑傲股市", "CAN-BASE-BREAKOUT")],
        entry=all_of(
            # 20 日振幅收缩到 15% 以内
            binary("lte",
                   binary("divide",
                          binary("subtract", rolling("rolling_max", HIGH, 20, 1),
                                 rolling("rolling_min", LOW, 20, 1)),
                          CLOSE),
                   lit(0.15)),
            # 5 日均量低于 20 日均量(缩量)
            binary("lt", rolling("rolling_mean", VOLUME, 5, 1),
                   rolling("rolling_mean", VOLUME, 20, 1)),
            # 当日放量 2 倍以上
            binary("gt", VOLUME,
                   binary("multiply", lit(2.0), rolling("rolling_mean", VOLUME, 20, 1))),
            # 收盘突破 20 日高点
            binary("gt", CLOSE, rolling("rolling_max", HIGH, 20, 1)),
        ),
        entry_reason="contracted_volume_breakout",
        exit_=binary("lt", CLOSE, rolling("rolling_min", LOW, 20, 1)),
        exit_reason="close_below_platform_low",
        fields=("close", "high", "low", "volume"),
    )),
    # 5. CAN-REV-05 横截面短期反转(组合)
    ("P1 横截面短期反转", {
        "schema_version": 1,
        "kind": "portfolio",
        "metadata": {
            "canonical_id": "CAN-REV-05",
            "evidence_status": "unverified",
            "hypothesis": (
                "过去 5 日跌幅最大的股票短期可能均值回复;按负收益排序每周轮动。"
                "来源名人归因不可信(SIM-GEN-001),仅作规则重新定义,不构成证据。"
            ),
            "sources": [src("股神西蒙斯，量化投资的数字奇迹", "SIM-GEN-001"),
                        src("Evidence-Based Technical Analysis", "EBTA-RULE-01")],
        },
        "universe": UNIVERSE,
        "data_requirements": requirements("close"),
        "entry": {"condition": lit(True), "reason_code": "eligible_for_ranking"},
        "native_exit": None,
        "positioning": {
            "type": "portfolio",
            "score": binary("multiply", lit(-1.0), indicator("return", CLOSE, 5)),
            "selection": {"n": 20, "type": "top_n"},
            "weighting": {"type": "equal"},
            "rebalance": {"frequency": "weekly", "interval_days": None},
            "risk_filter": None,
        },
        "holding": HOLDING,
        "overlays": overlays(),
        "portfolio_constraints": constraints(max_positions=20),
        "execution": EXECUTION,
        "validation": VALIDATION,
    }),
    # 6. CAN-REV-06 上升趋势内超卖恢复
    ("P1 趋势内超卖恢复", single_spec(
        canonical_id="CAN-REV-06",
        hypothesis="长期趋势向上时的短期超卖(RSI<30)可能均值修复，修复完成或趋势失效时退出。",
        sources=[src("量化交易从入门到精通", "QTP-003")],
        entry=all_of(
            binary("lt", indicator("rsi", CLOSE, 14), lit(30.0)),
            binary("gt", CLOSE, indicator("ma", CLOSE, 60)),
        ),
        entry_reason="uptrend_oversold",
        exit_=any_of(
            binary("gt", indicator("rsi", CLOSE, 14), lit(55.0)),
            binary("lt", CLOSE, indicator("ma", CLOSE, 60)),
        ),
        exit_reason="reversion_complete_or_trend_failed",
        fields=("close",),
    )),
    # 7. CAN-REG-02 量价齐升连续状态
    ("P1 量价齐升状态", single_spec(
        canonical_id="CAN-REG-02",
        hypothesis="价在 20 日均线上且相对放量的量价齐升状态可能短期延续，跌回均线下方表示状态结束。",
        sources=[src("简派交易", "SIMPLE-05")],
        entry=all_of(
            binary("gt", CLOSE, indicator("ma", CLOSE, 20)),
            binary("gt", rolling("volume_ratio", VOLUME, 20, 1), lit(1.2)),
        ),
        entry_reason="price_volume_confirm",
        exit_=binary("lt", CLOSE, indicator("ma", CLOSE, 20)),
        exit_reason="close_below_ma20",
        fields=("close", "volume"),
    )),
    # 8. CAN-REG-04 波动率状态交互:低波状态下的突破
    ("P1 低波状态突破", single_spec(
        canonical_id="CAN-REG-04",
        hypothesis="突破信号在低波动状态(ATR/收盘价处于 60 日低分位)下的质量可能不同于高波状态。",
        sources=[src("波动率交易", "VOL-REG-01"),
                 src("量化交易从入门到精通", "QTP-004")],
        entry=all_of(
            binary("gt", CLOSE, rolling("rolling_max", HIGH, 20, 1)),
            binary("lt",
                   rolling("rolling_rank",
                           binary("divide",
                                  {"op": "atr", "high": HIGH, "low": LOW,
                                   "close": CLOSE, "window": 14},
                                  CLOSE),
                           60, 1),
                   lit(0.3)),
        ),
        entry_reason="breakout_in_low_vol",
        exit_=CHANNEL_EXIT,
        exit_reason="close_below_prior_low",
        fields=("close", "high", "low"),
    )),
    # 9. CAN-EXIT-04 消融变体:突破 + ATR 移动止损覆盖层
    ("P1 突破 ATR 移动止损", single_spec(
        canonical_id="CAN-EXIT-04",
        hypothesis="固定入场序列下,3 倍 ATR 移动止损覆盖层相对无覆盖层的成本收益需要独立测量。",
        sources=[src("笑傲股市", "CAN-STOP-LOSS"),
                 src("股市趋势技术分析", "TREND-06"),
                 src("股票大作手回忆录", "LIV-06")],
        entry=binary("gt", CLOSE, rolling("rolling_max", HIGH, 20, 1)),
        entry_reason="close_above_prior_high",
        exit_=CHANNEL_EXIT,
        exit_reason="close_below_prior_low",
        fields=("close", "high", "low"),
        risk_overlay={
            "enabled": True, "type": "atr_multiple", "value": 3.0,
            "atr_period": 14, "trailing": True,
        },
    )),
]


def main() -> None:
    db = SessionLocal()
    try:
        admin_id = db.execute(
            text("SELECT id FROM users WHERE username = :u"),
            {"u": ADMIN_USERNAME},
        ).scalar()
        if not admin_id:
            raise SystemExit(f"users 表找不到用户 {ADMIN_USERNAME}")
        print(f"admin id: {admin_id}")

        available = BAR_FIELDS | snapshot_available_fields(db)
        existing = set(db.execute(
            text("SELECT name FROM quant_strategy WHERE owner_id = :o"),
            {"o": admin_id},
        ).scalars())

        created, skipped = 0, 0
        for name, raw in SPECS:
            if name in existing:
                print(f"跳过(已存在): {name}")
                skipped += 1
                continue
            capability = resolve_capabilities(raw, available_fields=available)
            if capability.status != CapabilityStatus.SUPPORTED:
                print(f"!! {name} 能力校验未通过: {capability.status}")
                for issue in capability.issues:
                    print(f"   - {issue.path}: {issue.message}")
                continue
            spec = with_status(parse_strategy_spec(raw), "unverified")
            strategy = Strategy(
                owner_id=admin_id,
                is_system=False,
                name=name,
                template="strategy_spec",
                kind=spec.kind,
                params={},
                spec_schema_version=spec.schema_version,
                spec=spec.model_dump(mode="json"),
                spec_hash=strategy_spec_hash(spec),
                research_status="unverified",
                enabled=True,
            )
            db.add(strategy)
            db.flush()
            print(f"创建: [{strategy.id}] {name} "
                  f"({spec.metadata.canonical_id}, hash={strategy.spec_hash[:12]}…)")
            created += 1
        db.commit()
        print(f"完成: 新建 {created}, 跳过 {skipped}, 总计 {len(SPECS)}")
    finally:
        db.close()


if __name__ == "__main__":
    main()
