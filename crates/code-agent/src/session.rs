use crate::agent::orchestrator::{OrchestratorAgent, OrchestratorRuntime};
use crate::agent::verifier::VerifierAgent;
use crate::agent::{LoopDetector, LoopLevel, ThinkStrategy, Verdict};
use crate::context::summary::estimate_tokens;
use crate::context::ContextManager;
use crate::retry::consume_stream_with_retry;
use crate::runtime::{
    build_run_summary_from as runtime_build_run_summary_from, emit_run_terminal, now_ts, RunState,
    ToolCallContext, ToolRuntime,
};
use crate::AgentEvent;
use anyhow::Result;
use code_tools::{PermissionConfig, PermissionGuard, PermissionMode, Tool, ToolRisk};
use hank_provider::{
    CompletionRequest, ContentBlock, LlmProvider, Message, Role, StopReason, ToolDefinition,
};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

const MAX_ITERATIONS: usize = 25;
const LLM_STREAM_TIMEOUT_SECS: u64 = 120;
/// 验证后最多允许修订的轮数（FR-VERIFY-2）
const MAX_REVISIONS: usize = 2;
/// MaxTokens 首次命中时静默提高的输出上限（【AF 08】8K→64K）
const MAX_OUTPUT_TOKENS_UPPER: u32 = 64_000;
/// 续写恢复消息最大注入次数（【AF 08】最多 3 次）
const MAX_CONTINUATIONS: u32 = 3;
/// 递减回报判停：续写增量低于该 token 数视为无进展（【AF 08】<500）
const CONTINUATION_MIN_GAIN_TOKENS: u32 = 500;

/// Agent execution mode
pub enum AgentMode {
    /// Simple flat loop (backward compatible, for simple queries)
    Simple,
    /// Orchestrated multi-agent with Think/Act/Observe
    Orchestrated { think_strategy: ThinkStrategy },
}

impl Default for AgentMode {
    fn default() -> Self {
        Self::Simple
    }
}

pub struct AgentSession {
    provider: Arc<dyn LlmProvider>,
    tools: Vec<Arc<dyn Tool>>,
    messages: Vec<Message>,
    system_prompt: String,
    model: String,
    tool_definitions: Vec<ToolDefinition>,
    mode: AgentMode,
    context_manager: ContextManager,
    /// 权限守卫（默认 workspace-write）
    permission: Arc<PermissionGuard>,
    /// 工作目录，用于 sandbox 路径校验与环境上下文
    work_dir: String,
    /// 分层上下文 debug 摘要：(segment 名称列表, 总字符数)。
    /// 设置后会在 run 开始时发出 ContextAssembled 事件（FR-CTX-9, FR-EVT-9）。
    context_summary: Option<(Vec<String>, usize)>,
    /// 写操作后是否启用 VerifierAgent 复核（FR-VERIFY-1/2）
    verify_after_write: bool,
    /// 原始用户请求（验证时用于传入 VerifierAgent）
    original_request: String,
    /// FR-TOOL-7: 延迟加载的工具名集合 — 初始只注册 stub，首次调用时注入完整 schema
    deferred_tool_names: std::collections::HashSet<String>,
    /// LLM 流超时（默认 120s；超时按失败收尾，不伪装成正常结束）
    stream_timeout: Duration,
}

impl AgentSession {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        tools: Vec<Arc<dyn Tool>>,
        model: String,
        system_prompt: String,
    ) -> Self {
        let tool_definitions = tools
            .iter()
            .map(|t| ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
            })
            .collect();
        let context_manager =
            ContextManager::with_provider(80_000, provider.clone(), model.clone());
        Self {
            provider,
            tools,
            messages: Vec::new(),
            system_prompt,
            model,
            tool_definitions,
            mode: AgentMode::Simple,
            context_manager,
            permission: Arc::new(PermissionGuard::with_defaults()),
            work_dir: String::new(),
            context_summary: None,
            verify_after_write: false,
            original_request: String::new(),
            deferred_tool_names: std::collections::HashSet::new(),
            stream_timeout: Duration::from_secs(LLM_STREAM_TIMEOUT_SECS),
        }
    }

    /// Create a session with orchestrated mode
    pub fn orchestrated(
        provider: Arc<dyn LlmProvider>,
        tools: Vec<Arc<dyn Tool>>,
        model: String,
        system_prompt: String,
        think_strategy: ThinkStrategy,
    ) -> Self {
        let tool_definitions = tools
            .iter()
            .map(|t| ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
            })
            .collect();
        let context_manager =
            ContextManager::with_provider(80_000, provider.clone(), model.clone());
        Self {
            provider,
            tools,
            messages: Vec::new(),
            system_prompt,
            model,
            tool_definitions,
            mode: AgentMode::Orchestrated { think_strategy },
            context_manager,
            permission: Arc::new(PermissionGuard::with_defaults()),
            work_dir: String::new(),
            context_summary: None,
            verify_after_write: false,
            original_request: String::new(),
            deferred_tool_names: std::collections::HashSet::new(),
            stream_timeout: Duration::from_secs(LLM_STREAM_TIMEOUT_SECS),
        }
    }

    /// 设置权限模式与工作目录（FR-PERM-1/4）。
    /// work_dir 既用于 sandbox 路径前缀校验，也作为默认可写根。
    pub fn with_permission(mut self, mode: PermissionMode, work_dir: impl Into<String>) -> Self {
        let work_dir = work_dir.into();
        let mut config = PermissionConfig::default();
        config.mode = mode;
        if !work_dir.is_empty() {
            config.sandbox_paths = vec![work_dir.clone()];
        }
        self.permission = Arc::new(PermissionGuard::new(config));
        self.work_dir = work_dir;
        self
    }

    /// 使用自定义权限配置
    pub fn with_permission_config(
        mut self,
        config: PermissionConfig,
        work_dir: impl Into<String>,
    ) -> Self {
        self.permission = Arc::new(PermissionGuard::new(config));
        self.work_dir = work_dir.into();
        self
    }

    /// 启用写操作后 VerifierAgent 复核（FR-VERIFY-1/2）。
    pub fn with_verification(mut self) -> Self {
        self.verify_after_write = true;
        self
    }

    /// 覆盖 LLM 流超时（主要用于测试；默认 120s）。
    /// 超时按失败收尾：发带原因的错误事件并终止 run（【SA 03】）。
    pub fn with_stream_timeout(mut self, timeout: Duration) -> Self {
        self.stream_timeout = timeout;
        self
    }

    /// 配置上下文总预算与压缩阈值（FR-BUDGET）。
    /// 默认 200K 预算 / 80K 阈值；换用更大上下文窗口的模型时可调高。
    /// 压缩阈值取预算的 40%，与默认比例一致。
    pub fn with_context_budget(mut self, total_budget: usize) -> Self {
        let threshold = (total_budget as f64 * 0.4) as usize;
        self.context_manager = ContextManager::with_budget(
            threshold,
            total_budget,
            self.provider.clone(),
            self.model.clone(),
        );
        self
    }

    /// FR-TOOL-7: 将指定工具标记为延迟加载。
    /// 初始 tool_definitions 中只有 stub（name+description，无详细 schema），
    /// 首次被 LLM 调用时动态注入完整 schema。
    pub fn with_deferred_tools(
        mut self,
        names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let deferred: std::collections::HashSet<String> =
            names.into_iter().map(|n| n.into()).collect();
        // 替换 deferred 工具的 tool_definition 为 stub（空 schema）
        for def in &mut self.tool_definitions {
            if deferred.contains(&def.name) {
                def.input_schema =
                    serde_json::json!({ "type": "object", "properties": {}, "required": [] });
            }
        }
        self.deferred_tool_names = deferred;
        self
    }

    /// 按分层组装系统提示词并记录 debug 摘要（FR-CTX-1/9）。
    /// 传入已组装好的分层（base/developer/environment/project），
    /// 由 prompt_pipe::build_layered_prompt 产出。
    pub fn with_layered_prompt(
        mut self,
        named_segments: Vec<crate::prompt_pipe::NamedSegment>,
    ) -> Self {
        let assembled = named_segments
            .iter()
            .map(|s| s.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let names: Vec<String> = named_segments.iter().map(|s| s.name.to_string()).collect();
        let total_chars = assembled.chars().count();
        self.system_prompt = assembled;
        self.context_summary = Some((names, total_chars));
        self
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn set_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
    }

    /// Run the agent loop, dispatching based on mode.
    pub async fn run(
        &mut self,
        user_content: Vec<ContentBlock>,
        event_tx: mpsc::Sender<AgentEvent>,
        cancel: CancellationToken,
    ) -> Result<()> {
        let run_id = uuid::Uuid::new_v4().to_string();

        // FR-EVT-2: run.started
        let _ = event_tx
            .send(AgentEvent::RunStarted {
                run_id: run_id.clone(),
                timestamp: now_ts(),
                cwd: if self.work_dir.is_empty() {
                    None
                } else {
                    Some(self.work_dir.clone())
                },
                model: self.model.clone(),
                permission_mode: self.permission.mode().as_str().to_string(),
                tools: self
                    .tool_definitions
                    .iter()
                    .map(|t| t.name.clone())
                    .collect(),
            })
            .await;

        // FR-CTX-9 / FR-EVT-9: context.assembled（仅 debug 摘要，不含完整 system prompt）
        if let Some((ref segments, total_chars)) = self.context_summary {
            let _ = event_tx
                .send(AgentEvent::ContextAssembled {
                    run_id: run_id.clone(),
                    turn_id: String::new(),
                    segments: segments.clone(),
                    total_chars,
                })
                .await;
        }

        // 记录原始请求用于验证阶段（FR-VERIFY-1）
        if self.original_request.is_empty() {
            self.original_request = user_content
                .iter()
                .filter_map(|b| {
                    if let ContentBlock::Text { text } = b {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
        }

        match &self.mode {
            AgentMode::Simple => {
                let mut run_state = RunState {
                    run_id: run_id.clone(),
                    ..Default::default()
                };
                let result = self
                    .run_simple(
                        user_content,
                        event_tx.clone(),
                        cancel.clone(),
                        &mut run_state,
                    )
                    .await;
                let paused = matches!(result, Ok(true));
                let plain: Result<()> = result.map(|_| ());
                emit_run_terminal(&run_id, &run_state, &plain, paused, &cancel, &event_tx).await;
                // 终态后统一发出 TurnComplete 关闭 SSE 流（在 RunCompleted 之后）
                let _ = event_tx.send(AgentEvent::TurnComplete).await;
                plain
            }
            AgentMode::Orchestrated { think_strategy } => {
                let think_strategy = think_strategy.clone();
                let mut run_state = RunState {
                    run_id: run_id.clone(),
                    ..Default::default()
                };
                let result = self
                    .run_orchestrated(
                        user_content,
                        event_tx.clone(),
                        cancel.clone(),
                        think_strategy,
                        &mut run_state,
                    )
                    .await;
                // Orchestrator 维护自身循环与 TurnComplete；这里只补 run 终态
                emit_run_terminal(&run_id, &run_state, &result, false, &cancel, &event_tx).await;
                result
            }
        }
    }

    /// Orchestrated mode: delegate to OrchestratorAgent
    async fn run_orchestrated(
        &mut self,
        user_content: Vec<ContentBlock>,
        event_tx: mpsc::Sender<AgentEvent>,
        cancel: CancellationToken,
        think_strategy: ThinkStrategy,
        run_state: &mut RunState,
    ) -> Result<()> {
        let mut orchestrator = OrchestratorAgent::new(
            self.provider.clone(),
            self.tools.clone(),
            self.model.clone(),
            self.system_prompt.clone(),
            think_strategy,
        );
        orchestrator.set_messages(std::mem::take(&mut self.messages));
        if !self.deferred_tool_names.is_empty() {
            orchestrator.set_deferred_tools(self.deferred_tool_names.clone());
        }
        let runtime = OrchestratorRuntime {
            run_id: run_state.run_id.clone(),
            work_dir: self.work_dir.clone(),
            permission: self.permission.clone(),
            verify_after_write: self.verify_after_write,
            original_request: self.original_request.clone(),
            stream_timeout: self.stream_timeout,
        };
        let result = orchestrator
            .run_with_state(user_content, event_tx, cancel, run_state, runtime)
            .await;
        self.messages = orchestrator.messages().to_vec();
        result
    }

    /// 验证阶段：写操作后调用 VerifierAgent，发 VerificationStarted/VerificationCompleted。
    /// 返回 issues（空=Approved/parse失败），needs_revision=true 时调用方可回注修订。
    async fn run_verify_phase(
        &self,
        run_id: &str,
        event_tx: &mpsc::Sender<AgentEvent>,
        cancel: CancellationToken,
        summary: &str,
    ) -> (Verdict, Vec<String>) {
        // 过滤出只读工具（FR-VERIFY-1）
        let readonly_tools: Vec<Arc<dyn Tool>> = self
            .tools
            .iter()
            .filter(|t| t.risk_level() == ToolRisk::Safe)
            .cloned()
            .collect();

        let _ = event_tx
            .send(AgentEvent::VerificationStarted {
                run_id: run_id.to_string(),
                command: None,
            })
            .await;

        let verifier =
            VerifierAgent::new(self.provider.clone(), readonly_tools, self.model.clone());
        let result = verifier
            .verify(&self.original_request, summary, event_tx.clone(), cancel)
            .await
            .unwrap_or_else(|e| {
                warn!("VerifierAgent error: {e}");
                crate::agent::VerificationResult {
                    verdict: Verdict::Approved,
                    issues: vec![format!("Verification failed: {e}")],
                }
            });

        let _ = event_tx
            .send(AgentEvent::VerificationCompleted {
                run_id: run_id.to_string(),
                verdict: result.verdict.clone(),
                issues: result.issues.clone(),
            })
            .await;

        (result.verdict, result.issues)
    }

    /// 取消/暂停路径的配对补全（【AF 17】）：把已收集的 tool_results 连同
    /// 未执行 tool_use 的占位错误 result 一起 push 进 messages，避免持久化的
    /// 消息序列以未配对的 assistant(tool_use) 结尾（恢复会话时第一次请求就 400）。
    /// `skip_id` 用于 ask_user：其 tool_use 不预填结果，由上层用用户答案回填。
    fn push_partial_tool_results(
        &mut self,
        assistant_content: &[ContentBlock],
        mut tool_results: Vec<ContentBlock>,
        skip_id: Option<&str>,
        placeholder: &str,
    ) {
        for block in assistant_content {
            if let ContentBlock::ToolUse { id, .. } = block {
                if Some(id.as_str()) == skip_id {
                    continue;
                }
                let has_result = tool_results.iter().any(|r| matches!(
                    r,
                    ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == id
                ));
                if !has_result {
                    tool_results.push(ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: placeholder.to_string(),
                        is_error: true,
                    });
                }
            }
        }
        if !tool_results.is_empty() {
            let msg = Message {
                role: Role::User,
                content: tool_results,
            };
            self.context_manager
                .add_pending(estimate_tokens(std::slice::from_ref(&msg)));
            self.messages.push(msg);
        }
    }

    /// Simple mode: flat stream → tools → loop (original behavior).
    /// 返回 Ok(true) 表示因 ask_user 暂停，Ok(false) 表示正常结束。
    /// 终态 TurnComplete 由 run() 在 RunCompleted 之后统一发出。
    async fn run_simple(
        &mut self,
        user_content: Vec<ContentBlock>,
        event_tx: mpsc::Sender<AgentEvent>,
        cancel: CancellationToken,
        run_state: &mut RunState,
    ) -> Result<bool> {
        let user_msg = Message {
            role: Role::User,
            content: user_content,
        };
        // 【SA 12】粗估增量：push 新消息后累加 pending，预算检查用 actual + pending
        self.context_manager
            .add_pending(estimate_tokens(std::slice::from_ref(&user_msg)));
        self.messages.push(user_msg);
        let mut revision_count = 0usize;

        // MaxTokens 恢复状态（【AF 08】三步递进 + 递减回报检测）
        let mut max_tokens_escalated = false;
        let mut continuation_count = 0u32;
        let mut small_output_streak = 0u32;
        let mut loop_detector = LoopDetector::new();
        let run_id = run_state.run_id.clone();
        let tool_runtime = ToolRuntime::new(
            self.permission.clone(),
            self.tools.clone(),
            self.work_dir.clone(),
        );

        for iteration in 0..MAX_ITERATIONS {
            if cancel.is_cancelled() {
                break;
            }

            // FR-LOOP-7 / FR-EVT-3: turn.started
            let turn_id = uuid::Uuid::new_v4().to_string();
            let _ = event_tx
                .send(AgentEvent::TurnStarted {
                    run_id: run_id.clone(),
                    turn_id: turn_id.clone(),
                    timestamp: now_ts(),
                    phase: "simple".to_string(),
                    message_count: self.messages.len(),
                })
                .await;

            let req = CompletionRequest {
                model: self.model.clone(),
                system: Some(self.system_prompt.clone()),
                messages: self.messages.clone(),
                tools: self.tool_definitions.clone(),
                max_tokens: if max_tokens_escalated {
                    MAX_OUTPUT_TOKENS_UPPER
                } else {
                    16384
                },
            };

            debug!(
                "Agent loop iteration {iteration}: model={}, messages={}",
                req.model,
                req.messages.len()
            );

            let llm_start = Instant::now();
            // 步骤级重试：发请求 + 消费流作为一个可重试单元（【SA 03】）
            let outcome = match consume_stream_with_retry(
                &self.provider,
                req,
                &event_tx,
                &cancel,
                self.stream_timeout,
            )
            .await
            {
                Ok(outcome) => outcome,
                Err(e) => {
                    error!("Stream error: {e}");
                    let _ = event_tx
                        .send(AgentEvent::Error {
                            message: e.to_string(),
                        })
                        .await;
                    return Err(e);
                }
            };
            let assistant_content = outcome.assistant_content;
            let stop_reason = outcome.stop_reason;
            let total_input_tokens = outcome.input_tokens;
            let total_output_tokens = outcome.output_tokens;
            let cancelled_during_stream = outcome.cancelled;

            // 超时是显式失败：发带原因的错误事件并终止 run，
            // 不能落入 EndTurn 正常收尾路径（【SA 03】）
            if outcome.timed_out {
                let message = format!(
                    "LLM stream timed out after {}s at iteration {iteration}",
                    self.stream_timeout.as_secs()
                );
                warn!("{message}");
                let _ = event_tx.send(AgentEvent::Error {
                    message: message.clone(),
                }).await;
                return Err(anyhow::anyhow!(message));
            }

            // Emit LLM metrics
            let latency_ms = llm_start.elapsed().as_millis() as u64;
            let _ = event_tx
                .send(AgentEvent::Metrics {
                    input_tokens: total_input_tokens,
                    output_tokens: total_output_tokens,
                    latency_ms,
                    model: self.model.clone(),
                    provider: self.provider.name().to_string(),
                    phase: Some("simple".to_string()),
                })
                .await;

            // 累积到 run 级别 usage（FR-EVT-2: run.completed.usage）
            run_state.peak_input_tokens = run_state.peak_input_tokens.max(total_input_tokens);
            run_state.output_tokens += total_output_tokens;

            // 更新实际 token 用量（provider 报告的 input_tokens 是整个上下文的大小）
            if total_input_tokens > 0 {
                self.context_manager
                    .update_actual_tokens(total_input_tokens as usize);
            }

            let assistant_msg = Message {
                role: Role::Assistant,
                content: assistant_content.clone(),
            };
            // 【SA 12】assistant 响应不在本次 actual 内，累加粗估增量
            self.context_manager
                .add_pending(estimate_tokens(std::slice::from_ref(&assistant_msg)));
            self.messages.push(assistant_msg);

            // Check budget after receiving assistant message
            match self.context_manager.check_budget(&self.messages) {
                crate::context::BudgetStatus::Overflow100 => {
                    warn!("Budget overflow, terminating at iteration {}", iteration);
                    break;
                }
                crate::context::BudgetStatus::Critical95 => {
                    let used = estimate_tokens(&self.messages);
                    let _ = event_tx
                        .send(AgentEvent::TokenWarning {
                            used_tokens: used,
                            total_budget: self.context_manager.total_budget(),
                            percent: 95,
                            action: "forcing_compression".to_string(),
                        })
                        .await;
                    if let Some(strategy) = self
                        .context_manager
                        .compress_async(&mut self.messages)
                        .await
                    {
                        let after = estimate_tokens(&self.messages);
                        // FR-BUDGET-6: 压缩后重置 actual tokens 避免旧值误判
                        self.context_manager.reset_actual_tokens();
                        let _ = event_tx
                            .send(AgentEvent::CompressionTriggered {
                                before_tokens: used,
                                after_tokens: after,
                                strategy: format!("{:?}", strategy),
                            })
                            .await;
                    }
                }
                crate::context::BudgetStatus::Warning80 => {
                    // P3-#18：与 orchestrator 对齐——80-95% 区间告警并按需压缩
                    let used = estimate_tokens(&self.messages);
                    debug!("Budget warning at 80%, compressing if needed");
                    let _ = event_tx
                        .send(AgentEvent::TokenWarning {
                            used_tokens: used,
                            total_budget: self.context_manager.total_budget(),
                            percent: 80,
                            action: "compress_if_needed".to_string(),
                        })
                        .await;
                    if self.context_manager.needs_compression(&self.messages) {
                        if let Some(strategy) = self
                            .context_manager
                            .compress_async(&mut self.messages)
                            .await
                        {
                            let after = estimate_tokens(&self.messages);
                            let _ = event_tx
                                .send(AgentEvent::CompressionTriggered {
                                    before_tokens: used,
                                    after_tokens: after,
                                    strategy: format!("{:?}", strategy),
                                })
                                .await;
                        }
                    }
                }
                _ => {}
            }

            // If cancelled during streaming, stop immediately
            if cancelled_during_stream {
                break;
            }

            // Handle MaxTokens: continue generation instead of stopping.
            // 方案 A（【AF 07】）：若截断前已有完整 tool_use 块（多工具调用被截断的
            // 常见情形），照常执行这些工具、push tool_results 后继续循环——
            // 保持 tool_use/tool_result 配对，不注入续写提示。
            let max_tokens_with_tools = stop_reason == StopReason::MaxTokens
                && assistant_content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolUse { .. }));
            if stop_reason == StopReason::MaxTokens && !max_tokens_with_tools {
                // 【AF 08】① 首次命中：静默提高 max_output_tokens 重试，不注入消息
                if !max_tokens_escalated {
                    warn!("MaxTokens hit at iteration {iteration}, escalating max_tokens to {MAX_OUTPUT_TOKENS_UPPER} and retrying");
                    max_tokens_escalated = true;
                    continue;
                }
                continuation_count += 1;
                // 递减回报检测：续写 ≥3 次且连续 2 次增量 <500 token → 直接停止
                if total_output_tokens < CONTINUATION_MIN_GAIN_TOKENS {
                    small_output_streak += 1;
                } else {
                    small_output_streak = 0;
                }
                if continuation_count > MAX_CONTINUATIONS
                    || (continuation_count >= MAX_CONTINUATIONS && small_output_streak >= 2)
                {
                    // ③ 认栽：发事件告知输出被截断
                    warn!("MaxTokens recovery exhausted at iteration {iteration}, output truncated");
                    let _ = event_tx
                        .send(AgentEvent::Error {
                            message: "Output truncated: the response hit the output token limit repeatedly and could not be completed. The partial output above is kept as-is.".to_string(),
                        })
                        .await;
                    break;
                }
                // ② 注入恢复消息（四要素：不要道歉、不要回顾、从断点直接继续、
                // 拆成更小的块），第二次起更强硬，最多 3 次
                let continuation_text = if continuation_count == 1 {
                    "[Your previous response was cut off by the output token limit. Do not apologize, do not recap. Continue directly from the exact point where you stopped, and break the remaining work into smaller chunks.]"
                } else {
                    "[Your response was cut off AGAIN by the output token limit. Do NOT apologize, do NOT restart or recap. Resume immediately from the cutoff point, and use much smaller chunks.]"
                };
                let continuation_msg = Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text {
                        text: continuation_text.to_string(),
                    }],
                };
                self.context_manager
                    .add_pending(estimate_tokens(std::slice::from_ref(&continuation_msg)));
                self.messages.push(continuation_msg);
                continue;
            }

            // If stop reason is tool_use (or MaxTokens with completed tool_use blocks),
            // execute tools and loop
            if stop_reason == StopReason::ToolUse || max_tokens_with_tools {
                continuation_count = 0;
                small_output_streak = 0;
                let mut tool_results: Vec<ContentBlock> = Vec::new();
                let mut ask_user_triggered = false;
                let mut pending_loop_nudge: Option<String> = None;
                let mut loop_terminated = false;
                let mut ask_user_tool_id: Option<String> = None;

                for block in &assistant_content {
                    if let ContentBlock::ToolUse { id, name, input } = block {
                        // Check cancellation before each tool
                        if cancel.is_cancelled() {
                            // 【AF 17】取消前补全配对：已执行的结果保留，
                            // 未执行的 tool_use 补占位错误 result
                            let results = std::mem::take(&mut tool_results);
                            self.push_partial_tool_results(
                                &assistant_content,
                                results,
                                None,
                                "cancelled before execution",
                            );
                            return Ok(false);
                        }

                        // FR-TOOL-7: 首次调用 deferred 工具时注入完整 schema
                        if self.deferred_tool_names.contains(name) {
                            if let Some(tool) = self.tools.iter().find(|t| t.name() == name) {
                                if let Some(def) =
                                    self.tool_definitions.iter_mut().find(|d| d.name == *name)
                                {
                                    def.input_schema = tool.input_schema();
                                }
                            }
                            self.deferred_tool_names.remove(name);
                            // 【AF 11】deferred loading 的意义是让模型拿到完整 schema 后再
                            // 构造参数：本次盲猜调用不执行，返回错误结果让模型重试
                            tool_results.push(ContentBlock::ToolResult {
                                tool_use_id: id.clone(),
                                content: "Tool schema now loaded, please retry with correct parameters".to_string(),
                                is_error: true,
                            });
                            continue;
                        }

                        // Detect ask_user tool — emit event and break
                        if name == "ask_user" {
                            let question =
                                input["question"].as_str().unwrap_or_default().to_string();
                            let options = input["options"]
                                .as_array()
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                        .collect()
                                })
                                .unwrap_or_default();
                            let _ = event_tx
                                .send(AgentEvent::AskUser {
                                    question,
                                    options,
                                    tool_use_id: id.clone(),
                                })
                                .await;
                            ask_user_triggered = true;
                            ask_user_tool_id = Some(id.clone());
                            break;
                        }

                        // Check for loop detection（【SA 03】相同调用+相同结果才算无进展）
                        let loop_level = loop_detector.record_and_check(name, input);
                        if loop_level != LoopLevel::None {
                            let pattern = loop_detector.loop_pattern();
                            let _ = event_tx
                                .send(AgentEvent::LoopDetected {
                                    pattern: pattern.clone(),
                                    window_size: loop_detector.window_size(),
                                })
                                .await;

                            if loop_level == LoopLevel::Breaker {
                                warn!("Loop detected: terminating agent loop");
                                tool_results.push(ContentBlock::ToolResult {
                                    tool_use_id: id.clone(),
                                    content: format!(
                                        "Loop detected: {}. Agent terminating to prevent infinite loop.",
                                        pattern
                                    ),
                                    is_error: true,
                                });
                                loop_terminated = true;
                                break;
                            }

                            // 未达熔断阈值（warning/critical）：记录 nudge，稍后追加到
                            // tool_results 消息末尾（critical 级响应，强制换路）。
                            // 不单独 push user 消息，以免插在 assistant(tool_use) 与
                            // tool_results 之间破坏配对。
                            pending_loop_nudge = Some(format!(
                                "⚠️ Loop detected: {}. Vary your approach or use different tools.",
                                pattern
                            ));
                        }

                        let result_block = tool_runtime
                            .execute_tool_call(
                                ToolCallContext {
                                    id: id.as_str(),
                                    name: name.as_str(),
                                    input,
                                    run_id: &run_id,
                                    turn_id: &turn_id,
                                },
                                &event_tx,
                                run_state,
                            )
                            .await;
                        // 回填结果指纹（【SA 03】：结果不同属正常探索，不计 streak）
                        if let ContentBlock::ToolResult { content, .. } = &result_block {
                            loop_detector.record_result(name, input, content);
                        }
                        tool_results.push(result_block);
                    }
                }

                // If ask_user was triggered, break the agent loop.
                // 返回 paused，由 run() 发出终态 TurnComplete 关闭 SSE 流。
                if ask_user_triggered {
                    // 【AF 17】持久化前补全配对：已执行的工具结果保留（防止恢复后
                    // 重复执行非幂等写操作），其余未执行的 tool_use 补占位。
                    // ask_user 的 tool_use 不预填结果——按 server 端恢复协议
                    // （chat.rs pending_ask_user），用户的回答会作为该 id 的
                    // tool_result 追加为新的 user 消息回填。
                    let results = std::mem::take(&mut tool_results);
                    self.push_partial_tool_results(
                        &assistant_content,
                        results,
                        ask_user_tool_id.as_deref(),
                        "skipped: ask_user interrupted before execution",
                    );
                    return Ok(true);
                }

                // 追加 loop nudge 到 tool_results 同一消息（critical 级响应，强制换路）
                if let Some(nudge) = pending_loop_nudge {
                    tool_results.push(ContentBlock::Text { text: nudge });
                }

                // 循环终止时给本条 assistant 消息中所有未执行的 tool_use
                // 各补一条错误 tool_result，保持配对（否则下一轮请求 400）
                if loop_terminated {
                    for block in &assistant_content {
                        if let ContentBlock::ToolUse { id, .. } = block {
                            let has_result = tool_results.iter().any(|r| matches!(
                                r,
                                ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == id
                            ));
                            if !has_result {
                                tool_results.push(ContentBlock::ToolResult {
                                    tool_use_id: id.clone(),
                                    content: "Loop detected, execution aborted".to_string(),
                                    is_error: true,
                                });
                            }
                        }
                    }
                }

                let tool_results_msg = Message {
                    role: Role::User,
                    content: tool_results,
                };
                // 【SA 12】粗估增量：工具结果不等到下一次 LLM 调用才被预算察觉
                self.context_manager
                    .add_pending(estimate_tokens(std::slice::from_ref(&tool_results_msg)));
                self.messages.push(tool_results_msg);

                // 循环检测终止：push 完整 tool_results 消息后退出外层 iteration 循环
                if loop_terminated {
                    break;
                }

                // Budget check after tool results to catch large tool outputs
                match self.context_manager.check_budget(&self.messages) {
                    crate::context::BudgetStatus::Overflow100 => {
                        warn!("Budget overflow after tool results, terminating");
                        let _ = event_tx
                            .send(AgentEvent::TurnCompleted {
                                run_id: run_id.clone(),
                                turn_id: turn_id.clone(),
                                timestamp: now_ts(),
                            })
                            .await;
                        break;
                    }
                    crate::context::BudgetStatus::Critical95 => {
                        let used = estimate_tokens(&self.messages);
                        if let Some(strategy) = self
                            .context_manager
                            .compress_async(&mut self.messages)
                            .await
                        {
                            let after = estimate_tokens(&self.messages);
                            // FR-BUDGET-6: 压缩后重置 actual tokens
                            self.context_manager.reset_actual_tokens();
                            let _ = event_tx
                                .send(AgentEvent::CompressionTriggered {
                                    before_tokens: used,
                                    after_tokens: after,
                                    strategy: format!("{:?}", strategy),
                                })
                                .await;
                        }
                    }
                    _ => {}
                }

                // FR-EVT-3: turn.completed（工具轮结束，loop 将继续下一轮 LLM 交互）
                let _ = event_tx
                    .send(AgentEvent::TurnCompleted {
                        run_id: run_id.clone(),
                        turn_id: turn_id.clone(),
                        timestamp: now_ts(),
                    })
                    .await;
            } else {
                // Turn complete（正常结束，终态 TurnComplete 由 run() 发出）
                let _ = event_tx
                    .send(AgentEvent::TurnCompleted {
                        run_id: run_id.clone(),
                        turn_id: turn_id.clone(),
                        timestamp: now_ts(),
                    })
                    .await;

                // ─── 验证阶段 (FR-VERIFY-1/2) ───
                // 有写操作、验证已启用、未达修订上限、未取消时触发
                if self.verify_after_write
                    && !run_state.file_changes.is_empty()
                    && revision_count < MAX_REVISIONS
                    && !cancel.is_cancelled()
                {
                    let summary = runtime_build_run_summary_from(run_state);
                    let (verdict, issues) = self
                        .run_verify_phase(&run_id, &event_tx, cancel.clone(), &summary)
                        .await;

                    match verdict {
                        Verdict::Approved => {
                            break;
                        }
                        Verdict::NeedsRevision => {
                            revision_count += 1;
                            // 将 issues 回注为新的 user turn，继续修订循环
                            let issues_text = issues.join("\n- ");
                            let revision_msg = Message {
                                role: Role::User,
                                content: vec![ContentBlock::Text {
                                    text: format!(
                                        "Verification found issues that need to be fixed:\n- {issues_text}\n\nPlease fix these issues."
                                    ),
                                }],
                            };
                            self.context_manager
                                .add_pending(estimate_tokens(std::slice::from_ref(&revision_msg)));
                            self.messages.push(revision_msg);
                            // 继续下一轮 iteration，不 break
                        }
                        Verdict::Rejected => {
                            // 终止循环，issues 将在 build_run_summary 中通过 file_changes 记录
                            // 在 run_state 中记录拒绝原因以便 summary 引用
                            for issue in &issues {
                                run_state
                                    .verification_issues
                                    .push(format!("rejected: {issue}"));
                            }
                            break;
                        }
                    }
                } else {
                    break;
                }
            }

            if iteration == MAX_ITERATIONS - 1 {
                // #15：达到迭代上限必须告知用户（【AF 08】停了、为什么停、下一步能做什么）
                warn!("Agent loop reached max iterations ({MAX_ITERATIONS})");
                run_state.termination_note =
                    Some(format!("reached max iterations ({MAX_ITERATIONS})"));
                let _ = event_tx
                    .send(AgentEvent::Error {
                        message: format!(
                            "Agent stopped: reached max iterations ({MAX_ITERATIONS}). The task may be incomplete; consider rephrasing it or breaking it into smaller steps."
                        ),
                    })
                    .await;
            }
        }

        Ok(false)
    }
}
