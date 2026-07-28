"""数据库策略升级为完整 StrategySpec，并为执行证据增加快照与指纹列。

`quant_strategy.spec` 是当前完整策略定义的唯一事实来源；本 revision 不创建
策略历史表，也不提供草稿、发布、回滚或版本恢复。`template` / `params` 仅为
迁移期兼容保留。

历史回测、计划、信号和评估在 0014 之前没有创建时的完整规格及数据指纹，迁移
不能拿“当前策略”冒充历史证据，因此新增证据列允许为空。0014 之后的运行路径
负责在创建/执行时写入。

Revision ID: 0014_dynamic_strategy_spec
Revises: 0013_research_plan
"""
from __future__ import annotations

import hashlib
import json
from copy import deepcopy
from datetime import datetime
from typing import Any

import sqlalchemy as sa
from alembic import op

revision: str = "0014_dynamic_strategy_spec"
down_revision: str | None = "0013_research_plan"
branch_labels = None
depends_on = None

SCHEMA_VERSION = 1

# 由 app/strategy/presets.py 的受严格 Schema 校验结果复制而来。迁移文件必须
# 独立、确定，不能导入未来可能变化的运行时代码。值在本 revision 发布后不可改。
SYSTEM_STRATEGY_SPECS: dict[str, dict[str, Any]] = json.loads(
    r'''{"breakout":{"data_requirements":[{"availability":"daily_close","field":"close","required":true},{"availability":"daily_close","field":"high","required":true},{"availability":"daily_close","field":"low","required":true}],"entry":{"condition":{"left":{"name":"close","op":"field"},"op":"gt","right":{"input":{"name":"high","op":"field"},"op":"rolling_max","shift":1,"window":20}},"reason_code":"close_above_prior_high"},"execution":{"buy_limit_policy":"reject","cost_model":"a_share_daily_v1","execution_time":"next_open","max_entry_premium":0.0,"missing_bar_policy":"reject_entry_retry_exit","sell_limit_policy":"retry","signal_time":"close","suspension_policy":"reject_entry_retry_exit"},"holding":{"allow_add":false,"allow_reduce":false,"cooldown_days":0,"risk_reentry":"native_reset"},"kind":"single","metadata":{"canonical_id":"CAN-TRD-02","evidence_status":"unverified","hypothesis":"收盘突破历史区间上沿后可能延续，跌破较短退出通道表示假设失效。","sources":[{"book":"股市趋势技术分析","candidate_id":"TREND-03"}]},"native_exit":{"condition":{"left":{"name":"close","op":"field"},"op":"lt","right":{"input":{"name":"low","op":"field"},"op":"rolling_min","shift":1,"window":10}},"reason_code":"close_below_prior_low"},"overlays":{"risk":{"atr_period":14,"enabled":false,"trailing":false,"type":"fixed_pct","value":0.08},"take_profit":{"atr_period":14,"enabled":false,"trailing":false,"type":"fixed_pct","value":0.2}},"portfolio_constraints":{"long_only":true,"max_positions":500,"max_single_weight":1.0,"max_total_weight":1.0},"positioning":{"target":1.0,"type":"binary"},"schema_version":1,"universe":{"exclude_st":true,"min_amount_avg20":0.0,"min_listing_days":60,"pool_id":2},"validation":{"baseline_ids":["buy_and_hold","equal_weight"],"locked_oos":true,"parameter_scans":[],"rejection_criteria":["no_net_oos_increment","unstable_parameters","capacity_failure"]}},"ma_cross":{"data_requirements":[{"availability":"daily_close","field":"close","required":true}],"entry":{"condition":{"left":{"input":{"name":"close","op":"field"},"op":"ma","window":5},"op":"gt","right":{"input":{"name":"close","op":"field"},"op":"ma","window":20}},"reason_code":"fast_ma_above_slow"},"execution":{"buy_limit_policy":"reject","cost_model":"a_share_daily_v1","execution_time":"next_open","max_entry_premium":0.0,"missing_bar_policy":"reject_entry_retry_exit","sell_limit_policy":"retry","signal_time":"close","suspension_policy":"reject_entry_retry_exit"},"holding":{"allow_add":false,"allow_reduce":false,"cooldown_days":0,"risk_reentry":"native_reset"},"kind":"single","metadata":{"canonical_id":"CAN-TRD-01","evidence_status":"unverified","hypothesis":"短期均价高于长期均价时，趋势延续概率可能高于简单持有基线。","sources":[{"book":"股市趋势技术分析","candidate_id":"TREND-08"}]},"native_exit":{"condition":{"left":{"input":{"name":"close","op":"field"},"op":"ma","window":5},"op":"lte","right":{"input":{"name":"close","op":"field"},"op":"ma","window":20}},"reason_code":"fast_ma_not_above_slow"},"overlays":{"risk":{"atr_period":14,"enabled":false,"trailing":false,"type":"fixed_pct","value":0.08},"take_profit":{"atr_period":14,"enabled":false,"trailing":false,"type":"fixed_pct","value":0.2}},"portfolio_constraints":{"long_only":true,"max_positions":500,"max_single_weight":1.0,"max_total_weight":1.0},"positioning":{"target":1.0,"type":"binary"},"schema_version":1,"universe":{"exclude_st":true,"min_amount_avg20":0.0,"min_listing_days":60,"pool_id":2},"validation":{"baseline_ids":["buy_and_hold","equal_weight"],"locked_oos":true,"parameter_scans":[],"rejection_criteria":["no_net_oos_increment","unstable_parameters","capacity_failure"]}},"mean_reversion":{"data_requirements":[{"availability":"daily_close","field":"close","required":true}],"entry":{"condition":{"args":[{"left":{"input":{"name":"close","op":"field"},"op":"rsi","window":14},"op":"lt","right":{"op":"literal","value":30.0}},{"left":{"name":"close","op":"field"},"op":"gt","right":{"input":{"name":"close","op":"field"},"op":"ma","window":60}}],"op":"all"},"reason_code":"uptrend_oversold"},"execution":{"buy_limit_policy":"reject","cost_model":"a_share_daily_v1","execution_time":"next_open","max_entry_premium":0.0,"missing_bar_policy":"reject_entry_retry_exit","sell_limit_policy":"retry","signal_time":"close","suspension_policy":"reject_entry_retry_exit"},"holding":{"allow_add":false,"allow_reduce":false,"cooldown_days":0,"risk_reentry":"native_reset"},"kind":"single","metadata":{"canonical_id":"CAN-REV-06","evidence_status":"unverified","hypothesis":"长期趋势向上时的短期超卖可能均值修复，修复完成或趋势失效时退出。","sources":[{"book":"量化交易从入门到精通","candidate_id":"QTP-003"}]},"native_exit":{"condition":{"args":[{"left":{"input":{"name":"close","op":"field"},"op":"rsi","window":14},"op":"gt","right":{"op":"literal","value":55.0}},{"left":{"name":"close","op":"field"},"op":"lt","right":{"input":{"name":"close","op":"field"},"op":"ma","window":60}}],"op":"any"},"reason_code":"reversion_complete_or_trend_failed"},"overlays":{"risk":{"atr_period":14,"enabled":false,"trailing":false,"type":"fixed_pct","value":0.08},"take_profit":{"atr_period":14,"enabled":false,"trailing":false,"type":"fixed_pct","value":0.2}},"portfolio_constraints":{"long_only":true,"max_positions":500,"max_single_weight":1.0,"max_total_weight":1.0},"positioning":{"target":1.0,"type":"binary"},"schema_version":1,"universe":{"exclude_st":true,"min_amount_avg20":0.0,"min_listing_days":60,"pool_id":2},"validation":{"baseline_ids":["buy_and_hold","equal_weight"],"locked_oos":true,"parameter_scans":[],"rejection_criteria":["no_net_oos_increment","unstable_parameters","capacity_failure"]}},"momentum_rotation":{"data_requirements":[{"availability":"daily_close","field":"close","required":true}],"entry":{"condition":{"op":"literal","value":true},"reason_code":"eligible_for_ranking"},"execution":{"buy_limit_policy":"reject","cost_model":"a_share_daily_v1","execution_time":"next_open","max_entry_premium":0.0,"missing_bar_policy":"reject_entry_retry_exit","sell_limit_policy":"retry","signal_time":"close","suspension_policy":"reject_entry_retry_exit"},"holding":{"allow_add":false,"allow_reduce":false,"cooldown_days":0,"risk_reentry":"native_reset"},"kind":"portfolio","metadata":{"canonical_id":"CAN-TRD-05","evidence_status":"unverified","hypothesis":"横截面中短期动量较强的股票可能延续，每周轮动并用短均线控制趋势失效。","sources":[{"book":"股票大作手回忆录","candidate_id":"LIV-04"}]},"native_exit":null,"overlays":{"risk":{"atr_period":14,"enabled":false,"trailing":false,"type":"fixed_pct","value":0.08},"take_profit":{"atr_period":14,"enabled":false,"trailing":false,"type":"fixed_pct","value":0.2}},"portfolio_constraints":{"long_only":true,"max_positions":10,"max_single_weight":1.0,"max_total_weight":1.0},"positioning":{"rebalance":{"frequency":"weekly","interval_days":null},"risk_filter":{"left":{"name":"close","op":"field"},"op":"lt","right":{"input":{"name":"close","op":"field"},"op":"ma","window":20}},"score":{"left":{"left":{"op":"literal","value":0.6},"op":"multiply","right":{"input":{"name":"close","op":"field"},"op":"momentum","window":20}},"op":"add","right":{"left":{"op":"literal","value":0.4},"op":"multiply","right":{"input":{"name":"close","op":"field"},"op":"momentum","window":60}}},"selection":{"n":10,"type":"top_n"},"type":"portfolio","weighting":{"type":"equal"}},"schema_version":1,"universe":{"exclude_st":true,"min_amount_avg20":0.0,"min_listing_days":60,"pool_id":2},"validation":{"baseline_ids":["buy_and_hold","equal_weight"],"locked_oos":true,"parameter_scans":[],"rejection_criteria":["no_net_oos_increment","unstable_parameters","capacity_failure"]}},"multifactor_hold":{"data_requirements":[{"availability":"daily_close","field":"close","required":true}],"entry":{"condition":{"op":"literal","value":true},"reason_code":"eligible_for_ranking"},"execution":{"buy_limit_policy":"reject","cost_model":"a_share_daily_v1","execution_time":"next_open","max_entry_premium":0.0,"missing_bar_policy":"reject_entry_retry_exit","sell_limit_policy":"retry","signal_time":"close","suspension_policy":"reject_entry_retry_exit"},"holding":{"allow_add":false,"allow_reduce":false,"cooldown_days":0,"risk_reentry":"native_reset"},"kind":"portfolio","metadata":{"canonical_id":"CAN-PORT-04","evidence_status":"unverified","hypothesis":"中短期动量与均线斜率的组合排序可能比单因子等权基线更稳定。","sources":[{"book":"打开量化投资的黑箱","candidate_id":"BLACKBOX-ALPHA-01"}]},"native_exit":null,"overlays":{"risk":{"atr_period":14,"enabled":false,"trailing":false,"type":"fixed_pct","value":0.08},"take_profit":{"atr_period":14,"enabled":false,"trailing":false,"type":"fixed_pct","value":0.2}},"portfolio_constraints":{"long_only":true,"max_positions":20,"max_single_weight":1.0,"max_total_weight":1.0},"positioning":{"rebalance":{"frequency":"monthly","interval_days":null},"risk_filter":null,"score":{"left":{"left":{"left":{"op":"literal","value":0.5},"op":"multiply","right":{"input":{"name":"close","op":"field"},"op":"momentum","window":20}},"op":"add","right":{"left":{"op":"literal","value":0.3},"op":"multiply","right":{"input":{"name":"close","op":"field"},"op":"momentum","window":60}}},"op":"add","right":{"left":{"op":"literal","value":0.2},"op":"multiply","right":{"input":{"input":{"name":"close","op":"field"},"op":"ma","window":20},"op":"return","window":5}}},"selection":{"n":20,"type":"top_n"},"type":"portfolio","weighting":{"type":"equal"}},"schema_version":1,"universe":{"exclude_st":true,"min_amount_avg20":0.0,"min_listing_days":60,"pool_id":2},"validation":{"baseline_ids":["buy_and_hold","equal_weight"],"locked_oos":true,"parameter_scans":[],"rejection_criteria":["no_net_oos_increment","unstable_parameters","capacity_failure"]}},"volume_breakout":{"data_requirements":[{"availability":"daily_close","field":"close","required":true},{"availability":"daily_close","field":"high","required":true},{"availability":"daily_close","field":"low","required":true},{"availability":"daily_close","field":"volume","required":true}],"entry":{"condition":{"args":[{"left":{"left":{"left":{"input":{"name":"high","op":"field"},"op":"rolling_max","shift":1,"window":20},"op":"subtract","right":{"input":{"name":"low","op":"field"},"op":"rolling_min","shift":1,"window":20}},"op":"divide","right":{"name":"close","op":"field"}},"op":"lte","right":{"op":"literal","value":0.15}},{"left":{"input":{"name":"volume","op":"field"},"op":"rolling_mean","shift":1,"window":5},"op":"lt","right":{"input":{"name":"volume","op":"field"},"op":"rolling_mean","shift":1,"window":20}},{"left":{"name":"volume","op":"field"},"op":"gt","right":{"left":{"op":"literal","value":2.0},"op":"multiply","right":{"input":{"name":"volume","op":"field"},"op":"rolling_mean","shift":1,"window":20}}},{"left":{"name":"close","op":"field"},"op":"gt","right":{"input":{"name":"high","op":"field"},"op":"rolling_max","shift":1,"window":20}}],"op":"all"},"reason_code":"contracted_volume_breakout"},"execution":{"buy_limit_policy":"reject","cost_model":"a_share_daily_v1","execution_time":"next_open","max_entry_premium":0.0,"missing_bar_policy":"reject_entry_retry_exit","sell_limit_policy":"retry","signal_time":"close","suspension_policy":"reject_entry_retry_exit"},"holding":{"allow_add":false,"allow_reduce":false,"cooldown_days":0,"risk_reentry":"native_reset"},"kind":"single","metadata":{"canonical_id":"CAN-TRD-04","evidence_status":"unverified","hypothesis":"价格和成交收缩后的放量向上突破可能形成趋势，平台下沿或 ATR 风险线失效。","sources":[{"book":"量化交易从入门到精通","candidate_id":"QTP-002"}]},"native_exit":{"condition":{"left":{"name":"close","op":"field"},"op":"lt","right":{"input":{"name":"low","op":"field"},"op":"rolling_min","shift":1,"window":20}},"reason_code":"close_below_platform_low"},"overlays":{"risk":{"atr_period":14,"enabled":true,"trailing":true,"type":"atr_multiple","value":2.0},"take_profit":{"atr_period":14,"enabled":false,"trailing":false,"type":"fixed_pct","value":0.2}},"portfolio_constraints":{"long_only":true,"max_positions":500,"max_single_weight":1.0,"max_total_weight":1.0},"positioning":{"target":1.0,"type":"binary"},"schema_version":1,"universe":{"exclude_st":true,"min_amount_avg20":0.0,"min_listing_days":60,"pool_id":2},"validation":{"baseline_ids":["buy_and_hold","equal_weight"],"locked_oos":true,"parameter_scans":[],"rejection_criteria":["no_net_oos_increment","unstable_parameters","capacity_failure"]}}}'''
)


def _canonical_json(spec: dict[str, Any]) -> str:
    return json.dumps(
        spec,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
        allow_nan=False,
    )


def _spec_hash(spec: dict[str, Any]) -> str:
    return hashlib.sha256(_canonical_json(spec).encode("utf-8")).hexdigest()


def upgrade() -> None:
    _preflight_legacy_strategies()
    _add_strategy_columns()
    _add_evidence_columns()
    _migrate_current_strategies()
    _tighten_strategy_columns()


def _preflight_legacy_strategies() -> None:
    """在任何自动提交 DDL 前验证所有 legacy 行都可确定性转换。"""
    if not SYSTEM_STRATEGY_SPECS:
        raise RuntimeError("0014 缺少固定的系统 StrategySpec 种子")

    strategy = sa.table(
        "quant_strategy",
        sa.column("id", sa.Integer()),
        sa.column("template", sa.String()),
        sa.column("params", sa.JSON()),
    )
    rows = op.get_bind().execute(sa.select(
        strategy.c.id, strategy.c.template, strategy.c.params,
    )).mappings().all()
    for row in rows:
        template = row["template"]
        base = SYSTEM_STRATEGY_SPECS.get(template)
        if base is None:
            raise RuntimeError(
                f"0014 无法迁移 quant_strategy.id={row['id']} 的未知模板 {template!r}"
            )
        params = row["params"] or {}
        if not isinstance(params, dict):
            raise RuntimeError(
                f"0014 无法迁移 quant_strategy.id={row['id']}: params 必须是 JSON 对象"
            )
        _apply_legacy_params(template, deepcopy(base), params)


def _add_strategy_columns() -> None:
    with op.batch_alter_table("quant_strategy") as batch:
        # 先可空添加并回填，避免对已有行伪造 JSON 默认值。
        batch.add_column(sa.Column("spec_schema_version", sa.Integer(), nullable=True))
        batch.add_column(sa.Column("spec", sa.JSON(), nullable=True))
        batch.add_column(sa.Column("spec_hash", sa.String(64), nullable=True))
        batch.add_column(sa.Column(
            "research_status", sa.String(32), nullable=False,
            server_default="unverified",
        ))
        batch.add_column(sa.Column("updated_at", sa.DateTime(), nullable=True))


def _add_evidence_columns() -> None:
    with op.batch_alter_table("quant_backtest_run") as batch:
        batch.add_column(sa.Column("strategy_spec_snapshot", sa.JSON(), nullable=True))
        batch.add_column(sa.Column("strategy_spec_hash", sa.String(64), nullable=True))
        batch.add_column(sa.Column("compiler_version", sa.String(64), nullable=True))
        batch.add_column(sa.Column("component_versions", sa.JSON(), nullable=True))
        batch.add_column(sa.Column("data_fingerprint", sa.String(64), nullable=True))
        batch.add_column(sa.Column("universe_fingerprint", sa.String(64), nullable=True))
        batch.add_column(sa.Column("cost_fingerprint", sa.String(64), nullable=True))
        batch.add_column(sa.Column("execution_fingerprint", sa.String(64), nullable=True))
        batch.create_index(
            "ix_quant_backtest_run_strategy_spec_hash", ["strategy_spec_hash"])
        batch.create_index(
            "ix_quant_backtest_run_execution_fingerprint", ["execution_fingerprint"])

    with op.batch_alter_table("quant_research_plan") as batch:
        batch.add_column(sa.Column("strategy_spec_snapshot", sa.JSON(), nullable=True))
        batch.add_column(sa.Column("strategy_spec_hash", sa.String(64), nullable=True))
        batch.create_index(
            "ix_quant_research_plan_strategy_spec_hash", ["strategy_spec_hash"])

    for table in ("quant_signal", "quant_strategy_eval"):
        with op.batch_alter_table(table) as batch:
            batch.add_column(sa.Column("spec_hash", sa.String(64), nullable=True))
            batch.create_index(f"ix_{table}_spec_hash", ["spec_hash"])


def _migrate_current_strategies() -> None:
    strategy = sa.table(
        "quant_strategy",
        sa.column("id", sa.Integer()),
        sa.column("template", sa.String()),
        sa.column("params", sa.JSON()),
        sa.column("created_at", sa.DateTime()),
        sa.column("spec_schema_version", sa.Integer()),
        sa.column("spec", sa.JSON()),
        sa.column("spec_hash", sa.String()),
        sa.column("updated_at", sa.DateTime()),
    )
    bind = op.get_bind()
    rows = bind.execute(sa.select(
        strategy.c.id,
        strategy.c.template,
        strategy.c.params,
        strategy.c.created_at,
    )).mappings().all()

    for row in rows:
        template = row["template"]
        base = SYSTEM_STRATEGY_SPECS.get(template)
        if base is None:
            raise RuntimeError(
                f"0014 无法迁移 quant_strategy.id={row['id']} 的未知模板 {template!r}"
            )
        params = row["params"] or {}
        if not isinstance(params, dict):
            raise RuntimeError(
                f"0014 无法迁移 quant_strategy.id={row['id']}: params 必须是 JSON 对象"
            )
        spec = _apply_legacy_params(template, deepcopy(base), params)
        bind.execute(
            strategy.update().where(strategy.c.id == row["id"]).values(
                spec_schema_version=SCHEMA_VERSION,
                spec=spec,
                spec_hash=_spec_hash(spec),
                updated_at=row["created_at"] or datetime.now(),
            )
        )


def _apply_legacy_params(
    template: str,
    spec: dict[str, Any],
    params: dict[str, Any],
) -> dict[str, Any]:
    """把 legacy 参数覆盖映射到完整规格。

    具体模板字段由固定的预置规格决定；映射与种子一起冻结在 revision 内，避免
    将来运行时默认值或编译器实现变化后重跑迁移得到不同数据。
    """
    def apply_overlays() -> None:
        for legacy, current in (
            ("risk_overlay", "risk"),
            ("take_profit", "take_profit"),
        ):
            if legacy not in params:
                continue
            value = params[legacy]
            if not isinstance(value, dict):
                raise RuntimeError(f"0014 无法迁移 {template}: {legacy} 必须是对象")
            unknown = set(value) - {"enabled", "type", "value", "atr_period"}
            if unknown:
                raise RuntimeError(
                    f"0014 无法迁移 {template}: {legacy} 包含未知字段 {sorted(unknown)}"
                )
            spec["overlays"][current].update(value)

    entry = spec["entry"]["condition"]
    native = spec["native_exit"]["condition"] if spec["native_exit"] else None

    if template == "ma_cross":
        fast = int(params.get("fast", 5))
        slow = int(params.get("slow", 20))
        entry["left"]["window"] = fast
        entry["right"]["window"] = slow
        native["left"]["window"] = fast
        native["right"]["window"] = slow
    elif template == "breakout":
        entry["right"]["window"] = int(params.get("entry", 20))
        native["right"]["window"] = int(params.get("exit", 10))
        spec["execution"]["max_entry_premium"] = float(
            params.get("max_entry_premium", 0.0))
    elif template == "mean_reversion":
        buy = float(params.get("rsi_buy", 30))
        sell = float(params.get("rsi_sell", 55))
        window = int(params.get("ma", 60))
        entry["args"][0]["right"]["value"] = buy
        entry["args"][1]["right"]["window"] = window
        native["args"][0]["right"]["value"] = sell
        native["args"][1]["right"]["window"] = window
    elif template == "volume_breakout":
        window = int(params.get("window", 20))
        entry["args"][0]["left"]["left"]["left"]["window"] = window
        entry["args"][0]["left"]["left"]["right"]["window"] = window
        entry["args"][0]["right"]["value"] = float(
            params.get("range_max", 0.15))
        entry["args"][1]["right"]["window"] = window
        entry["args"][2]["right"]["left"]["value"] = float(
            params.get("vol_mult", 2.0))
        entry["args"][2]["right"]["right"]["window"] = window
        entry["args"][3]["right"]["window"] = window
        native["right"]["window"] = window
        spec["overlays"]["risk"]["value"] = float(params.get("atr_mult", 2.0))
        spec["execution"]["max_entry_premium"] = float(
            params.get("max_entry_premium", 0.0))
    elif template == "momentum_rotation":
        top_n = int(params.get("top_n", 10))
        score = spec["positioning"]["score"]
        score["left"]["left"]["value"] = float(params.get("w_mom20", 0.6))
        score["right"]["left"]["value"] = float(params.get("w_mom60", 0.4))
        spec["positioning"]["selection"]["n"] = top_n
        spec["portfolio_constraints"]["max_positions"] = top_n
    elif template == "multifactor_hold":
        top_n = int(params.get("top_n", 20))
        spec["positioning"]["selection"]["n"] = top_n
        spec["portfolio_constraints"]["max_positions"] = top_n
    else:  # SYSTEM_STRATEGY_SPECS 的键集合变化时必须显式更新迁移映射。
        raise RuntimeError(f"0014 缺少模板 {template!r} 的 legacy 参数映射")

    apply_overlays()
    return spec


def _tighten_strategy_columns() -> None:
    with op.batch_alter_table("quant_strategy") as batch:
        batch.alter_column(
            "spec_schema_version", existing_type=sa.Integer(), nullable=False,
            server_default=str(SCHEMA_VERSION),
        )
        batch.alter_column("spec", existing_type=sa.JSON(), nullable=False)
        batch.alter_column("spec_hash", existing_type=sa.String(64), nullable=False)
        batch.alter_column("updated_at", existing_type=sa.DateTime(), nullable=False)
        batch.create_index("ix_quant_strategy_spec_hash", ["spec_hash"])
        batch.create_index("ix_quant_strategy_research_status", ["research_status"])


def downgrade() -> None:
    for table in ("quant_strategy_eval", "quant_signal"):
        with op.batch_alter_table(table) as batch:
            batch.drop_index(f"ix_{table}_spec_hash")
            batch.drop_column("spec_hash")

    with op.batch_alter_table("quant_research_plan") as batch:
        batch.drop_index("ix_quant_research_plan_strategy_spec_hash")
        batch.drop_column("strategy_spec_hash")
        batch.drop_column("strategy_spec_snapshot")

    with op.batch_alter_table("quant_backtest_run") as batch:
        batch.drop_index("ix_quant_backtest_run_execution_fingerprint")
        batch.drop_index("ix_quant_backtest_run_strategy_spec_hash")
        for column in (
            "execution_fingerprint",
            "cost_fingerprint",
            "universe_fingerprint",
            "data_fingerprint",
            "component_versions",
            "compiler_version",
            "strategy_spec_hash",
            "strategy_spec_snapshot",
        ):
            batch.drop_column(column)

    with op.batch_alter_table("quant_strategy") as batch:
        batch.drop_index("ix_quant_strategy_research_status")
        batch.drop_index("ix_quant_strategy_spec_hash")
        for column in (
            "updated_at",
            "research_status",
            "spec_hash",
            "spec",
            "spec_schema_version",
        ):
            batch.drop_column(column)
