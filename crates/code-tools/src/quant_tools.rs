use crate::{Tool, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use futures::StreamExt;
use hank_a2a_client::{A2aClient, Part, SendResult, StreamEvent, TaskState};
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::Duration;

use crate::quant_grant::QuantGrantStore;

/// 高成本 quant 工具执行超时（回测 / trial / 因子评估可能持续数分钟）。
const QUANT_LONG_TIMEOUT: Duration = Duration::from_secs(600);

/// 一次性构造全部 21 个 `quant_*` 工具。
pub fn quant_tools(
    base_url: impl Into<String>,
    token: impl Into<String>,
    session_id: impl Into<String>,
    source: impl Into<String>,
    grant_store: Arc<QuantGrantStore>,
) -> Vec<Arc<dyn Tool>> {
    let base_url = base_url.into();
    let token = token.into();
    let session_id = session_id.into();
    let source = source.into();

    let mk = |spec: ToolSpec, summary_fn: fn(&Value) -> String| {
        Arc::new(QuantTool::new(
            A2aClient::new(base_url.clone(), token.clone()),
            session_id.clone(),
            source.clone(),
            grant_store.clone(),
            spec,
            summary_fn,
        )) as Arc<dyn Tool>
    };

    vec![
        mk(
            ToolSpec {
                tool_name: "quant_catalog",
                skill: "catalog.get",
                description: "获取 quant 研究目录。编写 StrategySpec 前必须请求 strategy_authoring 段，复用完整示例并按精确操作符形状修改，禁止猜 schema。",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "sections": {
                            "type": "array",
                            "items": {
                                "type": "string",
                                "enum": [
                                    "product_boundary", "factors", "indicators",
                                    "filter_fields", "strategy_templates", "signals",
                                    "signal_sides", "manual_trade_sides",
                                    "signal_reason_types", "backtest_metrics",
                                    "strategy_authoring"
                                ]
                            },
                            "description": "编写 Spec 时使用 [\"strategy_authoring\", \"product_boundary\"]；省略则返回全部。"
                        }
                    }
                }),
                high_cost: false,
                artifact_name: "catalog",
            },
            |_| "获取研究目录".to_string(),
        ),
        mk(
            ToolSpec {
                tool_name: "quant_data_quality",
                skill: "market.data_quality",
                description: "获取全市场数据质量快照（覆盖度、告警级别）。高成本执行前建议先查。",
                input_schema: json!({ "type": "object", "properties": {} }),
                high_cost: false,
                artifact_name: "data_quality",
            },
            |_| "获取数据质量快照".to_string(),
        ),
        mk(
            ToolSpec {
                tool_name: "quant_validate_strategy",
                skill: "strategy.validate",
                description: "校验 StrategySpec 是否合法、当前系统是否支持，返回 capability 报告。",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "spec": { "type": "object", "description": "完整 StrategySpec" }
                    },
                    "required": ["spec"]
                }),
                high_cost: false,
                artifact_name: "validation_result",
            },
            |_| "校验策略规格".to_string(),
        ),
        mk(
            ToolSpec {
                tool_name: "quant_save_strategy_draft",
                skill: "strategy.save_draft",
                description: "保存策略草稿（enabled=false, research_status=unverified）。可带 parent_strategy_id 记录谱系。",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "策略名称" },
                        "spec": { "type": "object", "description": "完整 StrategySpec" },
                        "parent_strategy_id": { "type": ["integer", "string", "null"], "description": "父策略 ID，可选" }
                    },
                    "required": ["name", "spec"]
                }),
                high_cost: false,
                artifact_name: "strategy_draft",
            },
            |_| "保存策略草稿".to_string(),
        ),
        mk(
            ToolSpec {
                tool_name: "quant_create_experiment",
                skill: "experiment.create",
                description: "注册冻结规格的 experiment（必填 strategy_id）。不耗回测算力，无需确认。",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "hypothesis": { "type": "string" },
                        "permanent_candidate_id": { "type": "string" },
                        "strategy_id": { "type": ["integer", "string"], "description": "已保存策略 ID" },
                        "spec": { "type": ["object", "null"], "description": "可选，省略则用 strategy_id 当前 spec" },
                        "family_id": { "type": ["string", "null"] },
                        "universe_snapshot": { "type": ["object", "null"] },
                        "cost_snapshot": { "type": ["object", "null"] }
                    },
                    "required": ["title", "hypothesis", "permanent_candidate_id", "strategy_id"]
                }),
                high_cost: false,
                artifact_name: "experiment",
            },
            |_| "注册 experiment".to_string(),
        ),
        mk(
            ToolSpec {
                tool_name: "quant_get_experiment",
                skill: "experiment.get",
                description: "查询 experiment 详情、trials、multiplicity、pending promotions。",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "experiment_id": { "type": ["integer", "string"] }
                    },
                    "required": ["experiment_id"]
                }),
                high_cost: false,
                artifact_name: "experiment",
            },
            |_| "查询 experiment".to_string(),
        ),
        mk(
            ToolSpec {
                tool_name: "quant_list_experiments",
                skill: "experiment.list",
                description: "列出本人的 experiment 摘要。",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "include_archived": { "type": "boolean" },
                        "limit": { "type": "integer", "description": "默认 50，最大 50" }
                    }
                }),
                high_cost: false,
                artifact_name: "experiment_list",
            },
            |_| "列出 experiments".to_string(),
        ),
        mk(
            ToolSpec {
                tool_name: "quant_run_trial",
                skill: "experiment.trial",
                description: "在冻结 spec 上应用 param_patch 执行一次 trial 回测。默认优先 experiment 路径；模型应直接调用，系统确认闸门会自动暂停并询问用户，不要先调用 ask_user。",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "experiment_id": { "type": ["integer", "string"] },
                        "codes": { "type": "array", "items": { "type": "string" } },
                        "start": { "type": "string", "description": "YYYY-MM-DD" },
                        "end": { "type": "string", "description": "YYYY-MM-DD" },
                        "param_patch": { "type": "object", "description": "JSON Patch 风格路径 $.entry.window" },
                        "costs": { "type": "object" },
                        "pool_id": { "type": ["string", "integer", "null"] },
                        "dynamic_universe": { "type": "boolean" },
                        "confirmed": { "type": "boolean", "description": "由拦截层注入，模型勿填" },
                        "client_request_id": { "type": "string" }
                    },
                    "required": ["experiment_id"]
                }),
                high_cost: true,
                artifact_name: "trial_result",
            },
            |input| {
                let id = fmt_id(&input["experiment_id"]);
                let patch = input.get("param_patch").cloned().unwrap_or(json!({}));
                format!("试验 experiment {}，参数补丁 {}", id, patch)
            },
        ),
        mk(
            ToolSpec {
                tool_name: "quant_run_trial_batch",
                skill: "experiment.trial_batch",
                description: "顺序执行多组 param_patch trial（硬上限 8 组），按实际条数消耗授权；模型应直接调用，系统确认闸门会自动暂停并询问用户，不要先调用 ask_user。",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "experiment_id": { "type": ["integer", "string"] },
                        "codes": { "type": "array", "items": { "type": "string" } },
                        "start": { "type": "string" },
                        "end": { "type": "string" },
                        "param_patches": { "type": "array", "items": { "type": "object" }, "description": "参数补丁列表，最多 8 个" },
                        "costs": { "type": "object" },
                        "pool_id": { "type": ["string", "integer", "null"] },
                        "dynamic_universe": { "type": "boolean" },
                        "confirmed": { "type": "boolean", "description": "由拦截层注入，模型勿填" },
                        "client_request_id": { "type": "string" }
                    },
                    "required": ["experiment_id", "param_patches"]
                }),
                high_cost: true,
                artifact_name: "trial_batch_result",
            },
            |input| {
                let id = fmt_id(&input["experiment_id"]);
                let n = input["param_patches"].as_array().map(|a| a.len()).unwrap_or(0);
                format!("批量试验 experiment {}，共 {} 组参数", id, n)
            },
        ),
        mk(
            ToolSpec {
                tool_name: "quant_run_backtest",
                skill: "backtest.run",
                description: "对已保存 strategy_id 执行模拟回测（路径 S）；多轮提炼应优先 experiment.trial。模型应直接调用，系统确认闸门会自动暂停并询问用户，不要先调用 ask_user。",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "strategy_id": { "type": ["integer", "string"], "description": "必填，已保存策略 ID" },
                        "start": { "type": "string", "description": "YYYY-MM-DD" },
                        "end": { "type": "string", "description": "YYYY-MM-DD" },
                        "codes": { "type": "array", "items": { "type": "string" } },
                        "pool_id": { "type": ["string", "integer", "null"] },
                        "costs": {
                            "type": "object",
                            "properties": {
                                "commission": { "type": "number" },
                                "stamp_tax": { "type": "number" },
                                "slippage": { "type": "number", "description": "价格比例，如 5bps=0.0005" }
                            }
                        },
                        "confirmed": { "type": "boolean", "description": "由拦截层注入，模型勿填" },
                        "client_request_id": { "type": "string" }
                    },
                    "required": ["strategy_id", "start", "end"]
                }),
                high_cost: true,
                artifact_name: "backtest_summary",
            },
            |input| {
                let id = fmt_id(&input["strategy_id"]);
                let start = input["start"].as_str().unwrap_or("?");
                let end = input["end"].as_str().unwrap_or("?");
                let costs = input.get("costs").cloned().unwrap_or(json!("默认"));
                format!("回测策略 {}，区间 {} ~ {}，费用 {}", id, start, end, costs)
            },
        ),
        mk(
            ToolSpec {
                tool_name: "quant_get_backtest",
                skill: "backtest.get",
                description: "查询单个 backtest run 摘要（仅本人）。",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "run_id": { "type": "string" }
                    },
                    "required": ["run_id"]
                }),
                high_cost: false,
                artifact_name: "backtest_summary",
            },
            |_| "查询 backtest".to_string(),
        ),
        mk(
            ToolSpec {
                tool_name: "quant_list_backtests",
                skill: "backtest.list",
                description: "列出本人 backtest run 摘要。",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "strategy_id": { "type": ["integer", "string", "null"], "description": "可选，过滤某策略" },
                        "limit": { "type": "integer", "description": "默认 20，最大 50" },
                        "before_run_id": { "type": ["string", "null"], "description": "游标翻页" }
                    }
                }),
                high_cost: false,
                artifact_name: "backtest_list",
            },
            |_| "列出 backtests".to_string(),
        ),
        mk(
            ToolSpec {
                tool_name: "quant_screen",
                skill: "selection.screen",
                description: "结构化股票筛选（AND/OR 条件），返回命中列表与字段覆盖度。",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "date": { "type": "string" },
                        "pool_id": { "type": ["string", "integer", "null"] },
                        "logic": { "type": "string", "enum": ["and", "or"] },
                        "groups": { "type": "array", "items": { "type": "object" } },
                        "watchlist_only": { "type": "boolean" },
                        "limit": { "type": "integer", "description": "默认 100，A2A 硬上限 50" }
                    }
                }),
                high_cost: false,
                artifact_name: "screen_result",
            },
            |_| "执行结构化筛选".to_string(),
        ),
        mk(
            ToolSpec {
                tool_name: "quant_validate_factor",
                skill: "factor.validate",
                description: "校验因子表达式是否合法、系统是否支持。",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "expression": { "type": "object", "description": "因子表达式" }
                    },
                    "required": ["expression"]
                }),
                high_cost: false,
                artifact_name: "factor_validation",
            },
            |_| "校验因子表达式".to_string(),
        ),
        mk(
            ToolSpec {
                tool_name: "quant_preview_factor",
                skill: "factor.preview",
                description: "在少量标的（≤5）上计算因子序列，仅抽查，不代表全市场有效性。",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "expression": { "type": "object" },
                        "factor_key": { "type": ["string", "null"] },
                        "code": { "type": "string" },
                        "codes": { "type": "array", "items": { "type": "string" } },
                        "days": { "type": "integer", "description": "默认 60，最大 120" }
                    }
                }),
                high_cost: false,
                artifact_name: "factor_preview",
            },
            |_| "预览因子序列".to_string(),
        ),
        mk(
            ToolSpec {
                tool_name: "quant_evaluate_factor",
                skill: "factor.evaluate",
                description: "全市场（或指定池）因子有效性评估：IC/RankIC/ICIR（含 Newey-West t 值）、IC 衰减曲线、分层多空收益与多重检验提示。建议始终传 neutralize:[\"industry\",\"market_cap\"]——裸 IC 混着行业与市值暴露，低 PE 类因子的 IC 往往主要来自行业效应。模型应直接调用，系统确认闸门会自动暂停并询问用户，不要先调用 ask_user。",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "expression": { "type": ["object", "null"] },
                        "factor_key": { "type": ["string", "null"] },
                        "start": { "type": "string" },
                        "end": { "type": "string" },
                        "pool_id": { "type": ["string", "integer", "null"] },
                        "codes": { "type": "array", "items": { "type": "string" } },
                        "layers": { "type": "integer", "description": "默认 10，最大 10" },
                        "rebalance": { "type": "string", "enum": ["weekly", "monthly"], "description": "默认 weekly" },
                        "neutralize": {
                            "type": "array",
                            "items": { "type": "string", "enum": ["industry", "market_cap"] },
                            "description": "截面中性化维度。开启后 IC/分层/多空全部基于回归残差；省略为裸 IC，结论必须声明含行业与市值暴露。"
                        },
                        "horizons": {
                            "type": "array",
                            "items": { "type": "integer" },
                            "description": "前瞻期（交易日）列表，用于 IC 衰减曲线，默认 [1,5,10,20]，每个 1..60，最多 6 个。传 [] 关闭衰减计算。"
                        },
                        "confirmed": { "type": "boolean", "description": "由拦截层注入，模型勿填" },
                        "client_request_id": { "type": "string" }
                    },
                    "required": ["start", "end"]
                }),
                high_cost: true,
                artifact_name: "factor_evaluation",
            },
            |input| {
                let key = input["factor_key"].as_str().map(|s| s.to_string())
                    .or_else(|| input.get("expression").map(|v| format!("表达式:{}", v)))
                    .unwrap_or_else(|| "?".to_string());
                let start = input["start"].as_str().unwrap_or("?");
                let end = input["end"].as_str().unwrap_or("?");
                let layers = input["layers"].as_u64().unwrap_or(10);
                let neutral = input["neutralize"]
                    .as_array()
                    .filter(|a| !a.is_empty())
                    .map(|a| {
                        let modes: Vec<&str> = a.iter().filter_map(|v| v.as_str()).collect();
                        format!("，中性化 {}", modes.join("+"))
                    })
                    .unwrap_or_else(|| "，未中性化".to_string());
                format!(
                    "评估因子 {}，区间 {} ~ {}，{} 层分层{}",
                    key, start, end, layers, neutral
                )
            },
        ),
        mk(
            ToolSpec {
                tool_name: "quant_list_factor_evaluations",
                skill: "factor.evaluation_list",
                description: "列出本人历史因子评估（摘要：IC 头部指标、t 值、中性化口径、前瞻期）。多轮提炼必须用它做横向对比，不要靠对话记忆回忆上一轮跑了什么。",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "factor_key": { "type": ["string", "null"], "description": "按因子 key 过滤，可选" },
                        "status": { "type": "string", "enum": ["running", "done", "failed", "cancelled"] },
                        "limit": { "type": "integer", "description": "默认 20，最大 50" },
                        "before_id": { "type": ["integer", "string", "null"], "description": "游标分页" }
                    }
                }),
                high_cost: false,
                artifact_name: "factor_evaluation_list",
            },
            |input| match input["factor_key"].as_str() {
                Some(key) => format!("列出因子 {} 的历史评估", key),
                None => "列出历史因子评估".to_string(),
            },
        ),
        mk(
            ToolSpec {
                tool_name: "quant_get_factor_evaluation",
                skill: "factor.evaluation_get",
                description: "按 evaluation_id 取单次因子评估详情（含分层收益、IC 衰减曲线与多重检验报告）。",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "evaluation_id": { "type": ["integer", "string"], "description": "评估 ID" }
                    },
                    "required": ["evaluation_id"]
                }),
                high_cost: false,
                artifact_name: "factor_evaluation",
            },
            |input| {
                let id = input["evaluation_id"]
                    .as_i64()
                    .map(|v| v.to_string())
                    .or_else(|| input["evaluation_id"].as_str().map(|s| s.to_string()))
                    .unwrap_or_else(|| "?".to_string());
                format!("查看因子评估 {}", id)
            },
        ),
        mk(
            ToolSpec {
                tool_name: "quant_save_factor_draft",
                skill: "factor.save_draft",
                description: "保存因子草稿（enabled=false），仅 admin 可用。",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "key": { "type": "string" },
                        "name": { "type": "string" },
                        "expression": { "type": "object" },
                        "description": { "type": "string" },
                        "category": { "type": "string" }
                    },
                    "required": ["key", "name", "expression"]
                }),
                high_cost: false,
                artifact_name: "factor_draft",
            },
            |_| "保存因子草稿".to_string(),
        ),
        mk(
            ToolSpec {
                tool_name: "quant_gap_summary",
                skill: "system.gap_summary",
                description: "聚合系统能力缺口（审计失败 + research findings）。scope=global 仅 admin。",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "scope": { "type": "string", "enum": ["me", "global"], "description": "默认 me" },
                        "limit": { "type": "integer", "description": "默认 20，最大 50" },
                        "since_days": { "type": "integer", "description": "默认 30，最大 90" }
                    }
                }),
                high_cost: false,
                artifact_name: "gap_summary",
            },
            |_| "获取能力缺口摘要".to_string(),
        ),
        mk(
            ToolSpec {
                tool_name: "quant_report_finding",
                skill: "system.report_finding",
                description: "将真实系统缺口或工具明确判定的假说失败落表，供 gap_summary 聚合。仅证据不足、假说尚未被拒绝且无系统缺口时也要调用，但 findings 必须传空数组。",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "findings": {
                            "type": "array",
                            "description": "仅真实系统缺口或明确 hypothesis_rejected；证据不足但未拒绝时传 []。",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "kind": { "type": "string", "enum": ["missing_engine", "missing_data", "low_coverage", "product_gap", "ux_friction", "hypothesis_rejected"] },
                                    "detail": { "type": "string" },
                                    "evidence": { "type": "string" },
                                    "suggested_system_work": { "type": "string" },
                                    "experiment_id": { "type": ["integer", "string", "null"] },
                                    "run_id": { "type": ["integer", "string", "null"] }
                                },
                                "required": ["kind", "detail"]
                            }
                        },
                        "session_ref": { "type": "string" }
                    },
                    "required": ["findings"]
                }),
                high_cost: false,
                artifact_name: "report_result",
            },
            |_| "上报研究发现".to_string(),
        ),
    ]
}

#[derive(Clone)]
struct ToolSpec {
    tool_name: &'static str,
    skill: &'static str,
    description: &'static str,
    input_schema: Value,
    high_cost: bool,
    artifact_name: &'static str,
}

struct QuantTool {
    client: A2aClient,
    session_id: String,
    source: String,
    grant_store: Arc<QuantGrantStore>,
    spec: ToolSpec,
    summary_fn: fn(&Value) -> String,
}

impl QuantTool {
    fn new(
        client: A2aClient,
        session_id: String,
        source: String,
        grant_store: Arc<QuantGrantStore>,
        spec: ToolSpec,
        summary_fn: fn(&Value) -> String,
    ) -> Self {
        Self {
            client,
            session_id,
            source,
            grant_store,
            spec,
            summary_fn,
        }
    }

    fn metadata(&self) -> Option<Map<String, Value>> {
        let mut m = Map::new();
        m.insert("source".to_string(), Value::String(self.source.clone()));
        m.insert(
            "trace_session_id".to_string(),
            Value::String(self.session_id.clone()),
        );
        Some(m)
    }

    /// 高成本工具：无条件剥离模型自填的 confirmed，然后消耗一次授权并注入 confirmed=true。
    /// 授权不足时返回结构化错误，供拦截层二次确认。
    pub(crate) fn prepare_high_cost_payload(&self, mut input: Value) -> Result<Value, ToolOutput> {
        if let Some(obj) = input.as_object_mut() {
            obj.remove("confirmed");
        }
        if self.grant_store.consume(&self.session_id) {
            if let Some(obj) = input.as_object_mut() {
                obj.insert("confirmed".to_string(), Value::Bool(true));
            }
            Ok(input)
        } else {
            let summary = (self.summary_fn)(&input);
            Err(ToolOutput {
                content: json!({
                    "error": "quant_confirmation_required",
                    "summary": summary,
                    "note": "需要用户确认后才能执行高成本量化操作"
                })
                .to_string(),
                is_error: true,
            })
        }
    }
}

#[async_trait]
impl Tool for QuantTool {
    fn name(&self) -> &str {
        self.spec.tool_name
    }

    fn description(&self) -> &str {
        self.spec.description
    }

    fn input_schema(&self) -> Value {
        self.spec.input_schema.clone()
    }

    fn timeout(&self) -> Duration {
        if self.spec.high_cost {
            QUANT_LONG_TIMEOUT
        } else {
            crate::DEFAULT_TOOL_TIMEOUT
        }
    }

    fn needs_confirmation(&self, input: &Value) -> Option<crate::ConfirmationRequest> {
        if !self.spec.high_cost {
            return None;
        }
        if self.grant_store.remaining(&self.session_id) > 0 {
            return None;
        }
        Some(crate::ConfirmationRequest {
            summary: (self.summary_fn)(input),
            source: self.source.clone(),
        })
    }

    async fn execute(&self, input: Value) -> Result<ToolOutput> {
        let payload = if self.spec.high_cost {
            match self.prepare_high_cost_payload(input) {
                Ok(p) => p,
                Err(out) => return Ok(out),
            }
        } else {
            input
        };

        let metadata = self.metadata();
        if self.spec.high_cost {
            run_streaming(
                &self.client,
                self.spec.skill,
                payload,
                metadata,
                self.spec.artifact_name,
            )
            .await
        } else {
            run_short(
                &self.client,
                self.spec.skill,
                payload,
                metadata,
                self.spec.artifact_name,
            )
            .await
        }
    }
}

async fn run_short(
    client: &A2aClient,
    skill: &str,
    payload: Value,
    metadata: Option<Map<String, Value>>,
    _artifact_name: &str,
) -> Result<ToolOutput> {
    let msg = A2aClient::data_message(skill, payload, metadata);
    match client.send_message(msg).await? {
        SendResult::Task(task) => Ok(short_task_output(&task)),
        SendResult::Message(message) => {
            if let Some(data) = extract_data_part(&message.parts) {
                Ok(ToolOutput {
                    content: data.to_string(),
                    is_error: false,
                })
            } else {
                Ok(ToolOutput {
                    content: "A2A 返回的消息缺少 data part".to_string(),
                    is_error: true,
                })
            }
        }
    }
}

async fn run_streaming(
    client: &A2aClient,
    skill: &str,
    payload: Value,
    metadata: Option<Map<String, Value>>,
    artifact_name: &str,
) -> Result<ToolOutput> {
    let msg = A2aClient::data_message(skill, payload, metadata);
    let mut stream = client.send_streaming_message(msg).await?;
    let mut last_artifact: Option<Value> = None;
    let mut terminal_message: Option<String> = None;
    let mut terminal_state: Option<TaskState> = None;

    while let Some(event) = stream.next().await {
        let event = event?;
        match event {
            StreamEvent::Task { task } => {
                if task.status.state.is_terminal() {
                    terminal_state = Some(task.status.state);
                    terminal_message = task
                        .status
                        .message
                        .as_ref()
                        .and_then(|m| extract_data_part(&m.parts))
                        .map(|v| v.to_string());
                }
            }
            StreamEvent::StatusUpdate { status_update } => {
                if status_update.final_ || status_update.status.state.is_terminal() {
                    terminal_state = Some(status_update.status.state.clone());
                    terminal_message = status_update
                        .status
                        .message
                        .as_ref()
                        .and_then(|m| extract_data_part(&m.parts))
                        .map(|v| v.to_string());
                }
            }
            StreamEvent::ArtifactUpdate { artifact_update } => {
                let art = artifact_update.artifact;
                let matches_name = art.name.as_deref() == Some(artifact_name);
                if matches_name {
                    if let Some(data) = extract_data_part(&art.parts) {
                        last_artifact = Some(data);
                    }
                } else if last_artifact.is_none() {
                    // 兜底：在拿到具名 artifact 前，先缓存任意 data part
                    if let Some(data) = extract_data_part(&art.parts) {
                        last_artifact = Some(data);
                    }
                }
            }
        }
    }

    Ok(streaming_output(
        terminal_state,
        last_artifact,
        terminal_message,
    ))
}

/// 短任务 Task → ToolOutput。终态非 completed（failed/canceled/rejected）时标为错误，
/// 避免任务失败被模型读成成功结果。
fn short_task_output(task: &hank_a2a_client::Task) -> ToolOutput {
    let failed = task.status.state.is_terminal() && task.status.state != TaskState::Completed;
    // 优先取 artifacts 中的 data part
    if let Some(ref artifacts) = task.artifacts {
        for art in artifacts {
            if let Some(data) = extract_data_part(&art.parts) {
                return ToolOutput {
                    content: data.to_string(),
                    is_error: failed,
                };
            }
        }
    }
    // 其次取 status.message 中的 data part
    if let Some(ref message) = task.status.message {
        if let Some(data) = extract_data_part(&message.parts) {
            return ToolOutput {
                content: data.to_string(),
                is_error: failed,
            };
        }
    }
    ToolOutput {
        content: json!({ "status": task.status.state }).to_string(),
        is_error: failed,
    }
}

/// 流式长任务收尾：终态/最后 artifact/终态消息 → ToolOutput（纯函数，便于测试）。
///
/// - `Completed`：优先具名 artifact，其次终态消息，都没有才算错误；
/// - 其它终态（failed/canceled/rejected）：错误；
/// - `None`（流结束但没收到终态事件，如断连/服务端崩溃）：错误，
///   不得把已缓存的半截 artifact 当成功结果返回。
fn streaming_output(
    terminal_state: Option<TaskState>,
    last_artifact: Option<Value>,
    terminal_message: Option<String>,
) -> ToolOutput {
    match terminal_state {
        Some(TaskState::Completed) => {
            if let Some(data) = last_artifact {
                ToolOutput {
                    content: data.to_string(),
                    is_error: false,
                }
            } else if let Some(msg) = terminal_message {
                ToolOutput {
                    content: msg,
                    is_error: false,
                }
            } else {
                ToolOutput {
                    content: "长任务结束但未返回 artifact".to_string(),
                    is_error: true,
                }
            }
        }
        Some(state) => {
            let mut err = json!({
                "error": format!("任务终态: {:?}", state),
            });
            if let Some(msg) = terminal_message {
                err["message"] = msg.into();
            }
            ToolOutput {
                content: err.to_string(),
                is_error: true,
            }
        }
        None => ToolOutput {
            content: "SSE 流结束但未收到终态事件，任务状态未知（可能断连或服务端重启）。\
                      请用 quant_get_experiment / quant_get_backtest 恢复终态，不得盲目重发；\
                      确需重发必须复用同一 client_request_id。"
                .to_string(),
            is_error: true,
        },
    }
}

fn extract_data_part(parts: &[Part]) -> Option<Value> {
    for part in parts {
        if let Part::Data { data } = part {
            return Some(Value::Object(data.clone()));
        }
    }
    None
}

fn fmt_id(value: &Value) -> String {
    value
        .as_str()
        .map(|s| s.to_string())
        .or_else(|| value.as_i64().map(|n| n.to_string()))
        .or_else(|| value.as_u64().map(|n| n.to_string()))
        .unwrap_or_else(|| "?".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_confirmed_and_inject() {
        let store = Arc::new(QuantGrantStore::new());
        store.grant("s1", 1);
        let tool = QuantTool::new(
            A2aClient::new("http://x", "token"),
            "s1".to_string(),
            "trace_chat".to_string(),
            store.clone(),
            ToolSpec {
                tool_name: "quant_run_backtest",
                skill: "backtest.run",
                description: "",
                input_schema: json!({}),
                high_cost: true,
                artifact_name: "backtest_summary",
            },
            |_| "summary".to_string(),
        );
        let input = json!({
            "strategy_id": 42,
            "confirmed": true,
            "client_request_id": "req-1"
        });
        let payload = tool.prepare_high_cost_payload(input).unwrap();
        // 模型自填的 confirmed 被剥离，拦截层注入的 confirmed=true 被写入。
        assert_eq!(payload["confirmed"], true);
        assert_eq!(payload["strategy_id"], 42);
        assert_eq!(store.remaining("s1"), 0);
    }

    #[test]
    fn test_prepare_high_cost_payload_requires_grant() {
        let store = Arc::new(QuantGrantStore::new());
        let tool = QuantTool::new(
            A2aClient::new("http://x", "token"),
            "s1".to_string(),
            "trace_chat".to_string(),
            store.clone(),
            ToolSpec {
                tool_name: "quant_run_backtest",
                skill: "backtest.run",
                description: "",
                input_schema: json!({}),
                high_cost: true,
                artifact_name: "backtest_summary",
            },
            |_| "summary".to_string(),
        );
        let input = json!({ "strategy_id": 42, "confirmed": true });
        let err = tool.prepare_high_cost_payload(input).unwrap_err();
        assert!(err.is_error);
        assert!(err.content.contains("quant_confirmation_required"));
    }

    #[test]
    fn test_high_cost_needs_confirmation_without_grant() {
        let store = Arc::new(QuantGrantStore::new());
        let tool = QuantTool::new(
            A2aClient::new("http://x", "token"),
            "s1".to_string(),
            "trace_chat".to_string(),
            store,
            ToolSpec {
                tool_name: "quant_run_backtest",
                skill: "backtest.run",
                description: "",
                input_schema: json!({}),
                high_cost: true,
                artifact_name: "backtest_summary",
            },
            |input| {
                let id = fmt_id(&input["strategy_id"]);
                format!("回测策略 {}", id)
            },
        );
        let input = json!({ "strategy_id": 42 });
        let req = tool.needs_confirmation(&input).unwrap();
        assert_eq!(req.summary, "回测策略 42");
        assert_eq!(req.source, "trace_chat");
    }

    #[test]
    fn test_low_cost_never_needs_confirmation() {
        let store = Arc::new(QuantGrantStore::new());
        let tool = QuantTool::new(
            A2aClient::new("http://x", "token"),
            "s1".to_string(),
            "trace_chat".to_string(),
            store,
            ToolSpec {
                tool_name: "quant_catalog",
                skill: "catalog.get",
                description: "",
                input_schema: json!({}),
                high_cost: false,
                artifact_name: "catalog",
            },
            |_| "summary".to_string(),
        );
        assert!(tool.needs_confirmation(&json!({})).is_none());
    }

    #[test]
    fn test_catalog_schema_exposes_exact_authoring_section() {
        let tools = quant_tools(
            "http://x",
            "token",
            "s1",
            "trace_chat",
            Arc::new(QuantGrantStore::new()),
        );
        let catalog = tools
            .iter()
            .find(|tool| tool.name() == "quant_catalog")
            .unwrap();
        let schema = catalog.input_schema();
        let values = schema["properties"]["sections"]["items"]["enum"]
            .as_array()
            .unwrap();
        assert!(values.contains(&json!("strategy_authoring")));
        assert!(!values.contains(&json!("snippets")));
        assert!(!values.contains(&json!("operators")));
    }

    #[test]
    fn test_high_cost_tools_delegate_confirmation_to_runtime_gate() {
        let tools = quant_tools(
            "http://x",
            "token",
            "s1",
            "trace_chat",
            Arc::new(QuantGrantStore::new()),
        );
        for name in [
            "quant_run_trial",
            "quant_run_trial_batch",
            "quant_run_backtest",
            "quant_evaluate_factor",
        ] {
            let tool = tools.iter().find(|tool| tool.name() == name).unwrap();
            assert!(tool.description().contains("模型应直接调用"), "{name}");
            assert!(tool.description().contains("不要先调用 ask_user"), "{name}");
        }
    }

    #[test]
    fn test_factor_evaluation_read_tools_are_registered_and_low_cost() {
        let tools = quant_tools(
            "http://x",
            "token",
            "s1",
            "trace_chat",
            Arc::new(QuantGrantStore::new()),
        );
        // 只读记忆面不得走确认闸门,否则多轮对比每次都要用户点同意
        for name in [
            "quant_list_factor_evaluations",
            "quant_get_factor_evaluation",
        ] {
            let tool = tools.iter().find(|tool| tool.name() == name).unwrap();
            assert!(
                !tool.description().contains("模型应直接调用"),
                "{name} 不该带高成本话术"
            );
        }
        let get = tools
            .iter()
            .find(|tool| tool.name() == "quant_get_factor_evaluation")
            .unwrap();
        assert_eq!(
            get.input_schema()["required"],
            json!(["evaluation_id"]),
        );
    }

    #[test]
    fn test_evaluate_factor_exposes_neutralize_and_horizons() {
        let tools = quant_tools(
            "http://x",
            "token",
            "s1",
            "trace_chat",
            Arc::new(QuantGrantStore::new()),
        );
        let tool = tools
            .iter()
            .find(|tool| tool.name() == "quant_evaluate_factor")
            .unwrap();
        let schema = tool.input_schema();
        assert_eq!(
            schema["properties"]["neutralize"]["items"]["enum"],
            json!(["industry", "market_cap"]),
        );
        assert!(schema["properties"]["horizons"]["description"]
            .as_str()
            .unwrap()
            .contains("最多 6 个"));
        // 描述必须提示默认中性化,否则模型大概率省略该参数拿裸 IC 下结论
        assert!(tool.description().contains("neutralize"));
    }

    #[test]
    fn test_report_finding_description_keeps_inconclusive_results_out_of_gap_table() {
        let tools = quant_tools(
            "http://x",
            "token",
            "s1",
            "trace_chat",
            Arc::new(QuantGrantStore::new()),
        );
        let tool = tools
            .iter()
            .find(|tool| tool.name() == "quant_report_finding")
            .unwrap();
        assert!(tool.description().contains("findings 必须传空数组"));
        assert_eq!(
            tool.input_schema()["properties"]["findings"]["description"],
            "仅真实系统缺口或明确 hypothesis_rejected；证据不足但未拒绝时传 []。"
        );
    }

    #[test]
    fn test_extract_data_part_prefers_data() {
        let parts = vec![
            Part::Text {
                text: "hello".to_string(),
            },
            Part::Data {
                data: serde_json::from_value(json!({ "skill": "x", "payload": {} })).unwrap(),
            },
        ];
        let data = extract_data_part(&parts).unwrap();
        assert_eq!(data["skill"], "x");
    }

    #[test]
    fn test_streaming_output_completed_uses_artifact() {
        let out = streaming_output(
            Some(TaskState::Completed),
            Some(json!({ "sharpe": 1.2 })),
            None,
        );
        assert!(!out.is_error);
        assert!(out.content.contains("sharpe"));
    }

    #[test]
    fn test_streaming_output_no_terminal_is_error() {
        // 断连/服务端崩溃：即使缓存了半截 artifact 也不得返回成功
        let out = streaming_output(None, Some(json!({ "partial": true })), None);
        assert!(out.is_error);
        assert!(out.content.contains("未收到终态事件"));

        let out = streaming_output(None, None, Some("partial msg".to_string()));
        assert!(out.is_error);
    }

    #[test]
    fn test_streaming_output_failed_terminal_is_error() {
        let out = streaming_output(
            Some(TaskState::Failed),
            Some(json!({ "partial": true })),
            Some("boom".to_string()),
        );
        assert!(out.is_error);
        assert!(out.content.contains("Failed"));
        assert!(out.content.contains("boom"));
    }

    fn make_task(state: TaskState, with_artifact: bool) -> hank_a2a_client::Task {
        let artifacts = with_artifact.then(|| {
            vec![hank_a2a_client::Artifact {
                artifact_id: "a1".to_string(),
                name: Some("result".to_string()),
                description: None,
                parts: vec![Part::Data {
                    data: serde_json::from_value(json!({ "ok": true })).unwrap(),
                }],
                metadata: None,
                append: None,
                last_chunk: None,
            }]
        });
        hank_a2a_client::Task {
            id: "t1".to_string(),
            context_id: None,
            status: hank_a2a_client::TaskStatus {
                state,
                message: None,
                timestamp: None,
            },
            artifacts,
            history: None,
            metadata: None,
        }
    }

    #[test]
    fn test_short_task_output_failed_terminal_is_error() {
        // 有 artifact 但任务失败：结果必须标错误
        let out = short_task_output(&make_task(TaskState::Failed, true));
        assert!(out.is_error);

        // 无 artifact 的失败任务：status JSON 也必须标错误
        let out = short_task_output(&make_task(TaskState::Canceled, false));
        assert!(out.is_error);
        assert!(out.content.contains("canceled") || out.content.contains("Canceled"));
    }

    #[test]
    fn test_short_task_output_completed_is_success() {
        let out = short_task_output(&make_task(TaskState::Completed, true));
        assert!(!out.is_error);
        assert!(out.content.contains("ok"));
    }
}
