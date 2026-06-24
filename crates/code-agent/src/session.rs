use crate::agent::orchestrator::{OrchestratorAgent, OrchestratorRuntime};
use crate::agent::verifier::VerifierAgent;
use crate::agent::{LoopDetector, ThinkStrategy, Verdict};
use crate::context::summary::estimate_tokens;
use crate::context::ContextManager;
use crate::retry::stream_with_retry;
use crate::runtime::{
    build_run_summary_from as runtime_build_run_summary_from, emit_run_terminal, now_ts, RunState,
    ToolCallContext, ToolRuntime,
};
use crate::AgentEvent;
use anyhow::Result;
use code_tools::{PermissionConfig, PermissionGuard, PermissionMode, Tool, ToolRisk};
use hank_provider::{
    CompletionRequest, ContentBlock, LlmProvider, Message, Role, StopReason, StreamEvent,
    ToolDefinition,
};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

const MAX_ITERATIONS: usize = 25;
const LLM_STREAM_TIMEOUT_SECS: u64 = 120;
/// 验证后最多允许修订的轮数（FR-VERIFY-2）
const MAX_REVISIONS: usize = 2;

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
        self.messages.push(Message {
            role: Role::User,
            content: user_content,
        });
        let mut revision_count = 0usize;

        let mut consecutive_max_tokens = 0u32;
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
                max_tokens: 16384,
            };

            debug!(
                "Agent loop iteration {iteration}: model={}, messages={}",
                req.model,
                req.messages.len()
            );

            let llm_start = Instant::now();
            let mut stream = stream_with_retry(&self.provider, req).await?;

            let mut assistant_content: Vec<ContentBlock> = Vec::new();
            let mut current_text = String::new();
            let mut current_tool_id = String::new();
            let mut current_tool_name = String::new();
            let mut current_tool_input = String::new();
            let mut stop_reason = StopReason::EndTurn;
            let mut in_tool_block = false;
            let mut cancelled_during_stream = false;
            let mut total_input_tokens: u32 = 0;
            let mut total_output_tokens: u32 = 0;

            loop {
                let event = tokio::select! {
                    event = stream.next() => event,
                    _ = cancel.cancelled() => {
                        cancelled_during_stream = true;
                        None
                    }
                    _ = tokio::time::sleep(Duration::from_secs(LLM_STREAM_TIMEOUT_SECS)) => {
                        warn!("LLM stream timeout after {}s at iteration {}", LLM_STREAM_TIMEOUT_SECS, iteration);
                        None
                    }
                };
                let Some(event) = event else { break };
                match event {
                    Ok(StreamEvent::TextDelta(text)) => {
                        current_text.push_str(&text);
                        let _ = event_tx.send(AgentEvent::TextDelta { text }).await;
                    }
                    Ok(StreamEvent::ToolUseStart { id, name }) => {
                        debug!("ToolUseStart: id={id}, name={name}");
                        if !current_text.is_empty() {
                            assistant_content.push(ContentBlock::Text {
                                text: std::mem::take(&mut current_text),
                            });
                        }
                        current_tool_id = id;
                        current_tool_name = name;
                        current_tool_input.clear();
                        in_tool_block = true;
                    }
                    Ok(StreamEvent::ToolUseInputDelta(json)) => {
                        current_tool_input.push_str(&json);
                    }
                    Ok(StreamEvent::ToolUseEnd) => {
                        if !in_tool_block {
                            continue;
                        }
                        in_tool_block = false;
                        debug!("ToolUseEnd: id={current_tool_id}, name={current_tool_name}");
                        let input: serde_json::Value =
                            serde_json::from_str(&current_tool_input).unwrap_or_default();
                        assistant_content.push(ContentBlock::ToolUse {
                            id: std::mem::take(&mut current_tool_id),
                            name: std::mem::take(&mut current_tool_name),
                            input,
                        });
                        current_tool_input.clear();
                    }
                    Ok(StreamEvent::MessageEnd { stop_reason: sr }) => {
                        stop_reason = sr;
                    }
                    Ok(StreamEvent::Usage {
                        input_tokens,
                        output_tokens,
                    }) => {
                        total_input_tokens += input_tokens;
                        total_output_tokens += output_tokens;
                    }
                    Ok(StreamEvent::Error(msg)) => {
                        let _ = event_tx.send(AgentEvent::Error { message: msg }).await;
                    }
                    Err(e) => {
                        error!("Stream error: {e}");
                        let _ = event_tx
                            .send(AgentEvent::Error {
                                message: e.to_string(),
                            })
                            .await;
                        return Err(e);
                    }
                }
            }

            // Flush remaining text
            if !current_text.is_empty() {
                assistant_content.push(ContentBlock::Text {
                    text: std::mem::take(&mut current_text),
                });
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
            run_state.input_tokens = run_state.input_tokens.max(total_input_tokens);
            run_state.output_tokens += total_output_tokens;

            self.messages.push(Message {
                role: Role::Assistant,
                content: assistant_content.clone(),
            });

            // 更新实际 token 用量（provider 报告的 input_tokens 是整个上下文的大小）
            if total_input_tokens > 0 {
                self.context_manager
                    .update_actual_tokens(total_input_tokens as usize);
            }

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
                _ => {}
            }

            // If cancelled during streaming, stop immediately
            if cancelled_during_stream {
                break;
            }

            // Handle MaxTokens: continue generation instead of stopping
            if stop_reason == StopReason::MaxTokens {
                warn!("MaxTokens hit at iteration {iteration}, continuing generation");
                consecutive_max_tokens += 1;
                if consecutive_max_tokens >= 3 {
                    warn!("3 consecutive MaxTokens without tool use, treating as done");
                    break;
                }
                // Note: if in_tool_block was true, the partial tool call was never
                // pushed to assistant_content, so it's already discarded.
                // Assistant content already pushed above; inject continuation prompt
                self.messages.push(Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text {
                        text: "[Your previous response was cut off. Continue from where you left off.]".to_string(),
                    }],
                });
                continue;
            }

            // If stop reason is tool_use, execute tools and loop
            if stop_reason == StopReason::ToolUse {
                consecutive_max_tokens = 0;
                let mut tool_results: Vec<ContentBlock> = Vec::new();
                let mut ask_user_triggered = false;
                let mut pending_loop_nudge: Option<String> = None;

                for block in &assistant_content {
                    if let ContentBlock::ToolUse { id, name, input } = block {
                        // Check cancellation before each tool
                        if cancel.is_cancelled() {
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
                            break;
                        }

                        // Check for loop detection
                        if loop_detector.record_and_check(name, input) {
                            let pattern = loop_detector.loop_pattern();
                            let _ = event_tx
                                .send(AgentEvent::LoopDetected {
                                    pattern: pattern.clone(),
                                    window_size: 6,
                                })
                                .await;

                            if loop_detector.should_terminate(3) {
                                warn!("Loop detected: terminating agent loop");
                                tool_results.push(ContentBlock::ToolResult {
                                    tool_use_id: id.clone(),
                                    content: format!(
                                        "Loop detected: {}. Agent terminating to prevent infinite loop.",
                                        pattern
                                    ),
                                    is_error: true,
                                });
                                break;
                            }

                            // 未达终止阈值：记录 nudge，稍后追加到 tool_results 消息末尾
                            // （critical 级响应，强制换路）。不单独 push user 消息，
                            // 以免插在 assistant(tool_use) 与 tool_results 之间破坏配对。
                            pending_loop_nudge = Some(format!(
                                "⚠️ Loop detected: {}. Vary your approach or use different tools.",
                                pattern
                            ));
                        }

                        tool_results.push(
                            tool_runtime
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
                                .await,
                        );
                    }
                }

                // If ask_user was triggered, break the agent loop (don't push tool results).
                // 返回 paused，由 run() 发出终态 TurnComplete 关闭 SSE 流。
                if ask_user_triggered {
                    return Ok(true);
                }

                // 追加 loop nudge 到 tool_results 同一消息（critical 级响应，强制换路）
                if let Some(nudge) = pending_loop_nudge {
                    tool_results.push(ContentBlock::Text { text: nudge });
                }

                self.messages.push(Message {
                    role: Role::User,
                    content: tool_results,
                });

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
                            self.messages.push(Message {
                                role: Role::User,
                                content: vec![ContentBlock::Text {
                                    text: format!(
                                        "Verification found issues that need to be fixed:\n- {issues_text}\n\nPlease fix these issues."
                                    ),
                                }],
                            });
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
                warn!("Agent loop reached max iterations ({MAX_ITERATIONS})");
            }
        }

        Ok(false)
    }
}
