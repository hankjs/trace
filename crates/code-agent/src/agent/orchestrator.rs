use crate::agent::traits::{DelegatedTask, TaskStatus, ThinkStrategy};
use crate::agent::verifier::VerifierAgent;
use crate::agent::worker::WorkerAgent;
use crate::agent::LoopDetector;
use crate::agent::loop_detector::LoopLevel;
use crate::context::summary::{estimate_tokens, truncate_tool_result_default};
use crate::context::{BudgetStatus, ContextManager};
use crate::retry::{
    consume_stream_with_retry, emit_llm_request, emit_llm_response, stream_with_retry,
    LlmTraceContext,
};
use crate::runtime::{
    build_run_summary_from, classify_tool_error, now_ts, RunState, ToolCallContext, ToolGate,
    ToolRuntime,
};
use crate::AgentEvent;
use anyhow::Result;
use code_tools::{PermissionGuard, Tool, ToolOutput, ToolRisk};
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

const ORCHESTRATOR_MAX_ITERATIONS: usize = 50;
const DELEGATE_TASK_TOOL: &str = "delegate_task";
const LLM_STREAM_TIMEOUT_SECS: u64 = 120;
/// MaxTokens 首次命中时静默提高的输出上限（【AF 08】8K→64K）
const MAX_OUTPUT_TOKENS_UPPER: u32 = 64_000;
/// 续写恢复消息最大注入次数（【AF 08】最多 3 次）
const MAX_CONTINUATIONS: u32 = 3;
/// 递减回报判停：续写增量低于该 token 数视为无进展（【AF 08】<500）
const CONTINUATION_MIN_GAIN_TOKENS: u32 = 500;
const MAX_REVISIONS: usize = 2;

#[derive(Clone)]
pub(crate) struct OrchestratorRuntime {
    pub(crate) run_id: String,
    pub(crate) work_dir: String,
    pub(crate) permission: Arc<PermissionGuard>,
    pub(crate) verify_after_write: bool,
    pub(crate) original_request: String,
    /// LLM 流超时（由 session 透传；超时按失败收尾）
    pub(crate) stream_timeout: Duration,
}

pub struct OrchestratorAgent {
    provider: Arc<dyn LlmProvider>,
    tools: Vec<Arc<dyn Tool>>,
    model: String,
    system_prompt: String,
    tool_definitions: Vec<ToolDefinition>,
    think_strategy: ThinkStrategy,
    context_manager: ContextManager,
    loop_detector: LoopDetector,
    messages: Vec<Message>,
    /// MaxTokens 恢复状态（【AF 08】三步递进 + 递减回报检测）
    max_tokens_escalated: bool,
    max_tokens_continuations: u32,
    small_output_streak: u32,
    /// 当前 run 已进行的验证修订轮数（FR-VERIFY-1）
    verify_revision_count: usize,
    /// FR-TOOL-7: 延迟加载的工具名集合 — 初始只注册 stub，首次调用时注入完整 schema
    deferred_tool_names: std::collections::HashSet<String>,
}

impl OrchestratorAgent {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        tools: Vec<Arc<dyn Tool>>,
        model: String,
        system_prompt: String,
        think_strategy: ThinkStrategy,
    ) -> Self {
        let mut tool_definitions: Vec<ToolDefinition> = tools
            .iter()
            .map(|t| ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
            })
            .collect();

        // Add the delegate_task pseudo-tool
        tool_definitions.push(ToolDefinition {
            name: DELEGATE_TASK_TOOL.to_string(),
            description: "Delegate a sub-task to a worker agent. The worker will execute \
                the task independently and return a summary."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "description": {
                        "type": "string",
                        "description": "What the worker should accomplish"
                    },
                    "context": {
                        "type": "string",
                        "description": "Relevant context for the worker"
                    },
                    "tools_allowed": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Which tools the worker can use"
                    },
                    "affected_paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "File paths this task may modify"
                    }
                },
                "required": ["description", "context", "tools_allowed"]
            }),
        });

        let context_manager =
            ContextManager::with_provider(80_000, provider.clone(), model.clone());

        Self {
            provider,
            tools,
            model,
            system_prompt,
            tool_definitions,
            think_strategy,
            context_manager,
            loop_detector: LoopDetector::new(),
            messages: Vec::new(),
            max_tokens_escalated: false,
            max_tokens_continuations: 0,
            small_output_streak: 0,
            verify_revision_count: 0,
            deferred_tool_names: std::collections::HashSet::new(),
        }
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn set_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
    }

    /// FR-TOOL-7: 将指定工具标记为延迟加载。初始 tool_definitions 中只有 stub
    /// （name+description，空 schema），首次被 LLM 调用时动态注入完整 schema。
    pub fn set_deferred_tools(&mut self, names: std::collections::HashSet<String>) {
        for def in &mut self.tool_definitions {
            if names.contains(&def.name) {
                def.input_schema =
                    serde_json::json!({ "type": "object", "properties": {}, "required": [] });
            }
        }
        self.deferred_tool_names = names;
    }

    /// Run the orchestrator loop with Think/Act/Observe phases.
    pub async fn run(
        &mut self,
        user_content: Vec<ContentBlock>,
        event_tx: mpsc::Sender<AgentEvent>,
        cancel: CancellationToken,
    ) -> Result<()> {
        let mut run_state = RunState {
            run_id: uuid::Uuid::new_v4().to_string(),
            ..Default::default()
        };
        let runtime = OrchestratorRuntime {
            run_id: run_state.run_id.clone(),
            work_dir: String::new(),
            permission: Arc::new(PermissionGuard::with_defaults()),
            verify_after_write: false,
            original_request: user_content
                .iter()
                .filter_map(|b| {
                    if let ContentBlock::Text { text } = b {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join(" "),
            stream_timeout: Duration::from_secs(LLM_STREAM_TIMEOUT_SECS),
        };
        self.run_with_state(user_content, event_tx, cancel, &mut run_state, runtime)
            .await
    }

    pub(crate) async fn run_with_state(
        &mut self,
        user_content: Vec<ContentBlock>,
        event_tx: mpsc::Sender<AgentEvent>,
        cancel: CancellationToken,
        run_state: &mut RunState,
        runtime: OrchestratorRuntime,
    ) -> Result<()> {
        let user_msg = Message {
            role: Role::User,
            content: user_content,
        };
        // 【SA 12】粗估增量：push 新消息后累加 pending，预算检查用 actual + pending
        self.context_manager
            .add_pending(estimate_tokens(std::slice::from_ref(&user_msg)));
        self.messages.push(user_msg);

        // 每轮新 user query 进入 loop 时重置循环检测器（detector 是成员变量，
        // 跨 run 复用，【SA 03】要求新 query 清零 streak）
        self.loop_detector.reset();

        let mut _iterations_without_progress = 0;
        let mut last_worker_failed = false;

        for iteration in 0..ORCHESTRATOR_MAX_ITERATIONS {
            if cancel.is_cancelled() {
                let _ = event_tx.send(AgentEvent::TurnComplete).await;
                break;
            }

            // Budget check with multi-level strategy
            match self.context_manager.check_budget(&self.messages) {
                BudgetStatus::Overflow100 => {
                    warn!("Budget overflow at 100%, terminating agent loop");
                    let _ = event_tx.send(AgentEvent::TurnComplete).await;
                    break;
                }
                BudgetStatus::Critical95 => {
                    warn!("Budget critical at 95%, forcing compression");
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
                        self.context_manager.reset_actual_tokens(); // FR-BUDGET-6: 防止旧值误判
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
                BudgetStatus::Warning80 => {
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
                BudgetStatus::Normal => {
                    // No action needed
                }
            }

            // THINK phase (conditional)
            if self.should_think(iteration, last_worker_failed) {
                self.think_phase(&event_tx, &cancel, run_state, &runtime)
                    .await?;
                if cancel.is_cancelled() {
                    let _ = event_tx.send(AgentEvent::TurnComplete).await;
                    break;
                }
            }

            // ACT phase
            let act_result = self
                .act_phase(&event_tx, &cancel, run_state, &runtime)
                .await?;
            if cancel.is_cancelled() {
                let _ = event_tx.send(AgentEvent::TurnComplete).await;
                break;
            }

            match act_result {
                ActResult::Done => {
                    let _ = event_tx.send(AgentEvent::TurnComplete).await;
                    break;
                }
                ActResult::Continue => {
                    _iterations_without_progress += 1;
                    last_worker_failed = false;
                }
                ActResult::WorkerCompleted { success } => {
                    _iterations_without_progress = 0;
                    last_worker_failed = !success;
                }
            }

            if iteration == ORCHESTRATOR_MAX_ITERATIONS - 1 {
                // #15：达到迭代上限必须告知用户（【AF 08】停了、为什么停、下一步能做什么）
                warn!("Orchestrator reached max iterations");
                run_state.termination_note = Some(format!(
                    "reached max iterations ({ORCHESTRATOR_MAX_ITERATIONS})"
                ));
                let _ = event_tx
                    .send(AgentEvent::Error {
                        message: format!(
                            "Agent stopped: reached max iterations ({ORCHESTRATOR_MAX_ITERATIONS}). The task may be incomplete; consider rephrasing it or breaking it into smaller steps."
                        ),
                    })
                    .await;
                let _ = event_tx.send(AgentEvent::TurnComplete).await;
            }
        }

        Ok(())
    }

    fn should_think(&self, iteration: usize, last_worker_failed: bool) -> bool {
        match &self.think_strategy {
            ThinkStrategy::Always => true,
            ThinkStrategy::Never => false,
            ThinkStrategy::Conditional => iteration == 0 || last_worker_failed,
        }
    }

    /// Think phase: call LLM without tools to get structured reasoning.
    async fn think_phase(
        &mut self,
        event_tx: &mpsc::Sender<AgentEvent>,
        cancel: &CancellationToken,
        run_state: &mut RunState,
        runtime: &OrchestratorRuntime,
    ) -> Result<()> {
        let turn_id = uuid::Uuid::new_v4().to_string();
        let _ = event_tx
            .send(AgentEvent::TurnStarted {
                run_id: runtime.run_id.clone(),
                turn_id: turn_id.clone(),
                timestamp: now_ts(),
                phase: "think".to_string(),
                message_count: self.messages.len(),
            })
            .await;

        let req = CompletionRequest {
            model: self.model.clone(),
            system: Some(format!(
                "{}\n\nYou are in THINK mode. Analyze the situation and plan your next steps. \
                 Do NOT use tools. Just reason about what to do next.",
                self.system_prompt
            )),
            messages: self.messages.clone(),
            tools: vec![], // No tools in think phase
            max_tokens: 2048,
        };

        let llm_trace = LlmTraceContext {
            call_id: uuid::Uuid::new_v4().to_string(),
            run_id: Some(runtime.run_id.clone()),
            turn_id: Some(turn_id.clone()),
            model: req.model.clone(),
            provider: self.provider.name().to_string(),
            phase: "think".to_string(),
        };
        emit_llm_request(event_tx, &llm_trace, &req).await;

        debug!("Orchestrator THINK phase");
        let llm_start = Instant::now();
        let mut stream = stream_with_retry(&self.provider, req, event_tx, &llm_trace).await?;
        let mut think_text = String::new();
        let mut total_input_tokens = 0u32;
        let mut total_output_tokens = 0u32;
        let mut cache_read_tokens = 0u32;
        let mut cache_write_tokens = 0u32;
        let mut stop_reason = StopReason::EndTurn;
        let mut cancelled = false;
        let mut timed_out = false;

        loop {
            let event = tokio::select! {
                event = stream.next() => event,
                _ = cancel.cancelled() => {
                    cancelled = true;
                    None
                }
                _ = tokio::time::sleep(Duration::from_secs(LLM_STREAM_TIMEOUT_SECS)) => {
                    warn!("Think phase LLM stream timeout after {}s", LLM_STREAM_TIMEOUT_SECS);
                    timed_out = true;
                    None
                }
            };
            let Some(event) = event else { break };
            match event {
                Ok(StreamEvent::TextDelta(text)) => {
                    think_text.push_str(&text);
                    let _ = event_tx.send(AgentEvent::Thinking { text }).await;
                }
                Ok(StreamEvent::MessageEnd { stop_reason: sr }) => {
                    stop_reason = sr;
                    break;
                }
                Ok(StreamEvent::Usage {
                    input_tokens,
                    output_tokens,
                    cache_read_tokens: cache_read,
                    cache_write_tokens: cache_write,
                }) => {
                    total_input_tokens += input_tokens;
                    total_output_tokens += output_tokens;
                    cache_read_tokens += cache_read;
                    cache_write_tokens += cache_write;
                }
                Ok(StreamEvent::Error(msg)) => {
                    let _ = event_tx.send(AgentEvent::Error { message: msg }).await;
                    break;
                }
                Err(e) => {
                    error!("Think phase stream error: {e}");
                    break;
                }
                _ => {}
            }
        }

        let latency_ms = llm_start.elapsed().as_millis() as u64;
        emit_llm_response(
            event_tx,
            &llm_trace,
            vec![ContentBlock::Text {
                text: think_text.clone(),
            }],
            stop_reason,
            total_input_tokens,
            total_output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            latency_ms,
            cancelled,
            timed_out,
        )
        .await;

        // Add think output to messages as assistant turn
        if !think_text.is_empty() {
            let think_msg = Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text { text: think_text }],
            };
            self.context_manager
                .add_pending(estimate_tokens(std::slice::from_ref(&think_msg)));
            self.messages.push(think_msg);
        }

        let _ = event_tx
            .send(AgentEvent::Metrics {
                input_tokens: total_input_tokens,
                output_tokens: total_output_tokens,
                latency_ms,
                model: self.model.clone(),
                provider: self.provider.name().to_string(),
                phase: Some("think".to_string()),
            })
            .await;
        run_state.peak_input_tokens = run_state.peak_input_tokens.max(total_input_tokens);
        run_state.output_tokens += total_output_tokens;
        if total_input_tokens > 0 {
            self.context_manager
                .update_actual_tokens(total_input_tokens as usize);
        }
        let _ = event_tx
            .send(AgentEvent::TurnCompleted {
                run_id: runtime.run_id.clone(),
                turn_id,
                timestamp: now_ts(),
            })
            .await;

        Ok(())
    }

    /// Act phase: call LLM with tools, execute tools or delegate.
    async fn act_phase(
        &mut self,
        event_tx: &mpsc::Sender<AgentEvent>,
        cancel: &CancellationToken,
        run_state: &mut RunState,
        runtime: &OrchestratorRuntime,
    ) -> Result<ActResult> {
        let turn_id = uuid::Uuid::new_v4().to_string();
        let _ = event_tx
            .send(AgentEvent::TurnStarted {
                run_id: runtime.run_id.clone(),
                turn_id: turn_id.clone(),
                timestamp: now_ts(),
                phase: "act".to_string(),
                message_count: self.messages.len(),
            })
            .await;

        let req = CompletionRequest {
            model: self.model.clone(),
            system: Some(self.system_prompt.clone()),
            messages: self.messages.clone(),
            tools: self.tool_definitions.clone(),
            max_tokens: if self.max_tokens_escalated {
                MAX_OUTPUT_TOKENS_UPPER
            } else {
                16384
            },
        };

        let call_id = uuid::Uuid::new_v4().to_string();
        let llm_trace = LlmTraceContext {
            call_id: call_id.clone(),
            run_id: Some(runtime.run_id.clone()),
            turn_id: Some(turn_id.clone()),
            model: req.model.clone(),
            provider: self.provider.name().to_string(),
            phase: "act".to_string(),
        };

        debug!("Orchestrator ACT phase");
        let llm_start = Instant::now();
        // 步骤级重试：发请求 + 消费流作为一个可重试单元（【SA 03】）
        let outcome = consume_stream_with_retry(
            &self.provider,
            req,
            event_tx,
            cancel,
            runtime.stream_timeout,
            &llm_trace,
        )
        .await?;
        let assistant_content = outcome.assistant_content;
        let stop_reason = outcome.stop_reason;
        let total_input_tokens = outcome.input_tokens;
        let total_output_tokens = outcome.output_tokens;

        // 超时是显式失败：终止 run，不能落入 EndTurn 正常收尾路径（【SA 03】）
        if outcome.timed_out {
            let message = format!(
                "Act phase LLM stream timed out after {}s",
                runtime.stream_timeout.as_secs()
            );
            error!("{message}");
            let _ = event_tx.send(AgentEvent::Error {
                message: message.clone(),
            }).await;
            return Err(anyhow::anyhow!(message));
        }

        let assistant_msg = Message {
            role: Role::Assistant,
            content: assistant_content.clone(),
        };
        self.messages.push(assistant_msg);

        let latency_ms = llm_start.elapsed().as_millis() as u64;
        let _ = event_tx
            .send(AgentEvent::Metrics {
                input_tokens: total_input_tokens,
                output_tokens: total_output_tokens,
                latency_ms,
                model: self.model.clone(),
                provider: self.provider.name().to_string(),
                phase: Some("act".to_string()),
            })
            .await;
        run_state.peak_input_tokens = run_state.peak_input_tokens.max(total_input_tokens);
        run_state.output_tokens += total_output_tokens;
        if total_input_tokens > 0 {
            self.context_manager
                .update_actual_tokens(total_input_tokens as usize);
        }
        // 【SA 12】assistant 响应不在本次 actual 内，校准后累加粗估增量
        self.context_manager
            .add_pending(estimate_tokens(std::slice::from_ref(
                self.messages.last().unwrap(),
            )));

        // Handle MaxTokens: continue generation instead of stopping.
        // 方案 A（【AF 07】）：若截断前已有完整 tool_use 块，照常执行这些工具、
        // push tool_results 后继续循环，保持配对，不注入续写提示。
        let max_tokens_with_tools = stop_reason == StopReason::MaxTokens
            && assistant_content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { .. }));
        if stop_reason == StopReason::MaxTokens && !max_tokens_with_tools {
            // 【AF 08】① 首次命中：静默提高 max_output_tokens 重试，不注入消息
            if !self.max_tokens_escalated {
                warn!("Orchestrator MaxTokens hit, escalating max_tokens to {MAX_OUTPUT_TOKENS_UPPER} and retrying");
                self.max_tokens_escalated = true;
                let _ = event_tx
                    .send(AgentEvent::TurnCompleted {
                        run_id: runtime.run_id.clone(),
                        turn_id,
                        timestamp: now_ts(),
                    })
                    .await;
                return Ok(ActResult::Continue);
            }
            self.max_tokens_continuations += 1;
            // 递减回报检测：续写 ≥3 次且连续 2 次增量 <500 token → 直接停止
            if total_output_tokens < CONTINUATION_MIN_GAIN_TOKENS {
                self.small_output_streak += 1;
            } else {
                self.small_output_streak = 0;
            }
            if self.max_tokens_continuations > MAX_CONTINUATIONS
                || (self.max_tokens_continuations >= MAX_CONTINUATIONS
                    && self.small_output_streak >= 2)
            {
                // ③ 认栽：发事件告知输出被截断
                warn!("Orchestrator MaxTokens recovery exhausted, output truncated");
                let _ = event_tx
                    .send(AgentEvent::Error {
                        message: "Output truncated: the response hit the output token limit repeatedly and could not be completed. The partial output above is kept as-is.".to_string(),
                    })
                    .await;
                let _ = event_tx
                    .send(AgentEvent::TurnCompleted {
                        run_id: runtime.run_id.clone(),
                        turn_id,
                        timestamp: now_ts(),
                    })
                    .await;
                return Ok(ActResult::Done);
            }
            // ② 注入恢复消息（四要素：不要道歉、不要回顾、从断点直接继续、
            // 拆成更小的块），第二次起更强硬，最多 3 次
            let continuation_text = if self.max_tokens_continuations == 1 {
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
            let _ = event_tx
                .send(AgentEvent::TurnCompleted {
                    run_id: runtime.run_id.clone(),
                    turn_id,
                    timestamp: now_ts(),
                })
                .await;
            return Ok(ActResult::Continue);
        }

        // Reset counters on successful tool use
        self.max_tokens_continuations = 0;
        self.small_output_streak = 0;

        if stop_reason != StopReason::ToolUse && !max_tokens_with_tools {
            if runtime.verify_after_write && !run_state.file_changes.is_empty() {
                let _ = event_tx
                    .send(AgentEvent::VerificationStarted {
                        run_id: runtime.run_id.clone(),
                        command: None,
                    })
                    .await;
                let readonly_tools: Vec<Arc<dyn Tool>> = self
                    .tools
                    .iter()
                    .filter(|t| t.risk_level() == ToolRisk::Safe)
                    .cloned()
                    .collect();
                let verifier =
                    VerifierAgent::new(self.provider.clone(), readonly_tools, self.model.clone());
                let summary = build_run_summary_from(run_state);
                let result = verifier
                    .verify(
                        &runtime.run_id,
                        &runtime.original_request,
                        &summary,
                        event_tx.clone(),
                        cancel.clone(),
                    )
                    .await
                    .unwrap_or_else(|e| {
                        warn!("Orchestrator verifier error: {e}");
                        crate::agent::VerificationResult {
                            verdict: crate::agent::Verdict::Approved,
                            issues: vec![format!("Verification failed: {e}")],
                        }
                    });
                let _ = event_tx
                    .send(AgentEvent::VerificationCompleted {
                        run_id: runtime.run_id.clone(),
                        verdict: result.verdict.clone(),
                        issues: result.issues.clone(),
                    })
                    .await;
                match result.verdict {
                    crate::agent::Verdict::Approved => {
                        self.verify_revision_count = 0;
                    }
                    crate::agent::Verdict::NeedsRevision
                        if self.verify_revision_count < MAX_REVISIONS =>
                    {
                        // 注入修订请求，返回 Continue 让外层循环重新执行（FR-VERIFY-1/2）
                        self.verify_revision_count += 1;
                        let issue_text = result.issues.join("\n");
                        let revision_msg = Message {
                            role: Role::User,
                            content: vec![ContentBlock::Text {
                                text: format!(
                                    "Verification issues found (revision {}/{MAX_REVISIONS}):\n{issue_text}\nPlease fix these issues.",
                                    self.verify_revision_count
                                ),
                            }],
                        };
                        self.context_manager
                            .add_pending(estimate_tokens(std::slice::from_ref(&revision_msg)));
                        self.messages.push(revision_msg);
                        let _ = event_tx
                            .send(AgentEvent::TurnCompleted {
                                run_id: runtime.run_id.clone(),
                                turn_id,
                                timestamp: now_ts(),
                            })
                            .await;
                        return Ok(ActResult::Continue);
                    }
                    _ => {
                        // Rejected 或已达最大修订轮数
                        for issue in &result.issues {
                            run_state
                                .verification_issues
                                .push(format!("verification {:?}: {issue}", result.verdict));
                        }
                        self.verify_revision_count = 0;
                    }
                }
            }
            let _ = event_tx
                .send(AgentEvent::TurnCompleted {
                    run_id: runtime.run_id.clone(),
                    turn_id,
                    timestamp: now_ts(),
                })
                .await;
            return Ok(ActResult::Done);
        }

        // Execute tools: parallel for read-only, sequential for writes
        let mut tool_results: Vec<ContentBlock> = Vec::new();
        let mut had_worker = false;
        let mut worker_success = true;
        let mut pending_loop_nudge: Option<String> = None;

        // Separate tool calls into delegate tasks and regular tools
        let mut regular_tools: Vec<(&str, &str, &serde_json::Value)> = Vec::new();
        let mut delegate_tasks: Vec<(&str, &serde_json::Value)> = Vec::new();

        for block in &assistant_content {
            if let ContentBlock::ToolUse { id, name, input } = block {
                if name == DELEGATE_TASK_TOOL {
                    delegate_tasks.push((id.as_str(), input));
                } else {
                    regular_tools.push((id.as_str(), name.as_str(), input));
                }
            }
        }

        // FR-TOOL-7: 首次调用 deferred 工具时注入完整 schema（影响后续请求）。
        // 【AF 11】deferred loading 的意义是让模型拿到完整 schema 后再构造参数：
        // 本轮用空 schema 盲猜的调用不执行，返回错误结果让模型重试。
        let mut just_loaded_deferred: std::collections::HashSet<&str> =
            std::collections::HashSet::new();
        if !self.deferred_tool_names.is_empty() {
            for (_, name, _) in &regular_tools {
                if self.deferred_tool_names.remove(*name) {
                    if let Some(tool) = self.tools.iter().find(|t| t.name() == *name) {
                        let schema = tool.input_schema();
                        if let Some(def) =
                            self.tool_definitions.iter_mut().find(|d| d.name == *name)
                        {
                            def.input_schema = schema;
                        }
                    }
                    just_loaded_deferred.insert(*name);
                }
            }
        }

        // Check for loops on regular tools（【SA 03】相同调用+相同结果才算无进展）
        for (id, name, input) in &regular_tools {
            let loop_level = self.loop_detector.record_and_check(name, input);
            if loop_level != LoopLevel::None {
                let pattern = self.loop_detector.loop_pattern();
                let _ = event_tx
                    .send(AgentEvent::LoopDetected {
                        pattern: pattern.clone(),
                        window_size: self.loop_detector.window_size(),
                    })
                    .await;

                if loop_level == LoopLevel::Breaker {
                    warn!("Loop detection: terminating after repeated loops");
                    // 终止前给本条 assistant 消息中所有 tool_use 补全 tool_result
                    // （含当前及未执行的 regular/delegate 调用），保持配对
                    tool_results.push(ContentBlock::ToolResult {
                        tool_use_id: id.to_string(),
                        content: format!(
                            "Loop detected: {}. Agent terminating to prevent infinite loop.",
                            pattern
                        ),
                        is_error: true,
                    });
                    for remaining in &assistant_content {
                        if let ContentBlock::ToolUse { id: other_id, .. } = remaining {
                            let has_result = tool_results.iter().any(|r| matches!(
                                r,
                                ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == other_id
                            ));
                            if !has_result {
                                tool_results.push(ContentBlock::ToolResult {
                                    tool_use_id: other_id.clone(),
                                    content: "Loop detected, execution aborted".to_string(),
                                    is_error: true,
                                });
                            }
                        }
                    }
                    // push 完整 tool_results 消息后直接终止本轮 Act，
                    // 不再执行剩余工具与 delegate_tasks
                    let abort_msg = Message {
                        role: Role::User,
                        content: tool_results,
                    };
                    self.context_manager
                        .add_pending(estimate_tokens(std::slice::from_ref(&abort_msg)));
                    self.messages.push(abort_msg);
                    let _ = event_tx
                        .send(AgentEvent::TurnCompleted {
                            run_id: runtime.run_id.clone(),
                            turn_id,
                            timestamp: now_ts(),
                        })
                        .await;
                    return Ok(ActResult::Done);
                }

                // 未达终止阈值：记录 nudge，稍后追加到 tool_results 消息末尾。
                // 不单独 push user 消息，以免插在 assistant(tool_use) 与
                // tool_results 之间破坏配对（参照 session.rs 的方案）。
                pending_loop_nudge = Some(format!(
                    "⚠️ Loop detected: {}. Vary your approach or use different tools.",
                    pattern
                ));
            }
        }

        // 分区：本轮刚加载 schema 的 deferred 调用返回重试提示（不执行盲猜参数），
        // 其余照常执行
        let mut executable: Vec<(&str, &str, &serde_json::Value)> = Vec::new();
        for (id, name, input) in regular_tools {
            if just_loaded_deferred.contains(name) {
                tool_results.push(ContentBlock::ToolResult {
                    tool_use_id: id.to_string(),
                    content: "Tool schema now loaded, please retry with correct parameters"
                        .to_string(),
                    is_error: true,
                });
            } else {
                executable.push((id, name, input));
            }
        }

        // Execute regular tools — parallel if all are read-only, sequential otherwise
        if !executable.is_empty() {
            let has_writes = executable
                .iter()
                .any(|(_, name, _)| self.tools.iter().any(|t| t.name() == *name && t.is_write()));

            if !has_writes && executable.len() > 1 {
                // Parallel execution for read-only tools
                let results = self
                    .execute_tools_parallel(
                        &executable,
                        &event_tx,
                        cancel,
                        run_state,
                        runtime,
                        &turn_id,
                        &call_id,
                    )
                    .await;
                // 回填结果指纹（【SA 03】：结果不同属正常探索，不计 streak）
                for (id, name, input) in &executable {
                    if let Some(ContentBlock::ToolResult { content, .. }) = results.iter().find(
                        |r| matches!(r, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == id),
                    ) {
                        self.loop_detector.record_result(name, input, content);
                    }
                }
                tool_results.extend(results);
            } else {
                // Sequential execution for write tools
                for (id, name, input) in &executable {
                    if cancel.is_cancelled() {
                        break;
                    }
                    let result = self
                        .execute_single_tool(
                            id,
                            name,
                            input,
                            &event_tx,
                            run_state,
                            runtime,
                            &turn_id,
                            &call_id,
                        )
                        .await;
                    // 回填结果指纹
                    if let ContentBlock::ToolResult { content, .. } = &result {
                        self.loop_detector.record_result(name, input, content);
                    }
                    tool_results.push(result);
                }
            }
        }

        // Execute delegate tasks — parallel if no affected_paths conflicts, sequential otherwise
        if !delegate_tasks.is_empty() {
            had_worker = true;
            for (id, input) in &delegate_tasks {
                let _ = event_tx
                    .send(AgentEvent::ToolStart {
                        id: (*id).to_string(),
                        name: DELEGATE_TASK_TOOL.to_string(),
                        input: serde_json::to_string(input).unwrap_or_default(),
                        run_id: Some(runtime.run_id.clone()),
                        turn_id: Some(turn_id.clone()),
                        call_id: Some(call_id.clone()),
                        risk: Some("Delegation".to_string()),
                        timeout_ms: None,
                    })
                    .await;
            }

            // Detect write conflicts: tasks sharing any affected_path must run sequentially
            let tasks_parsed: Vec<(String, DelegatedTask)> = delegate_tasks
                .iter()
                .map(|(id, input)| {
                    let task = DelegatedTask {
                        id: uuid::Uuid::new_v4().to_string(),
                        description: input
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unnamed task")
                            .to_string(),
                        context: input
                            .get("context")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        tools_allowed: input
                            .get("tools_allowed")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        affected_paths: input
                            .get("affected_paths")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    };
                    (id.to_string(), task)
                })
                .collect();

            let has_path_conflict = {
                let mut seen_paths: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                let mut conflict = false;
                for (_, task) in &tasks_parsed {
                    for path in &task.affected_paths {
                        if !seen_paths.insert(path.clone()) {
                            conflict = true;
                            break;
                        }
                    }
                    if conflict {
                        break;
                    }
                }
                conflict
            };

            let has_write_workers = tasks_parsed.iter().any(|(_, task)| {
                task.tools_allowed.iter().any(|t_name| {
                    self.tools
                        .iter()
                        .any(|t| t.name() == t_name && t.is_write())
                })
            });

            // 并发条件：无写工具、无路径冲突、且多任务。写工具或路径冲突一律串行，
            // 避免并行写竞争（FR-CTX-8）。Worker 经 ToolRuntime 受同一权限边界约束（FR-PERM-6）。
            if !has_write_workers && !has_path_conflict && tasks_parsed.len() > 1 {
                // Parallel: read-only tasks with no path conflicts
                let futures: Vec<_> = tasks_parsed
                    .into_iter()
                    .map(|(id, task)| {
                        let worker_tools: Vec<Arc<dyn Tool>> = self
                            .tools
                            .iter()
                            .filter(|t| task.tools_allowed.contains(&t.name().to_string()))
                            .cloned()
                            .collect();
                        let worker = WorkerAgent::new(
                            self.provider.clone(),
                            worker_tools,
                            self.model.clone(),
                            runtime.permission.clone(),
                            runtime.work_dir.clone(),
                        );
                        let event_tx2 = event_tx.clone();
                        let cancel2 = cancel.clone();
                        async move {
                            let _ = event_tx2
                                .send(AgentEvent::WorkerSpawned {
                                    task_id: task.id.clone(),
                                    description: task.description.clone(),
                                })
                                .await;
                            let result =
                                worker.execute_task(&task, event_tx2.clone(), cancel2).await;
                            (id, result)
                        }
                    })
                    .collect();
                let results = futures::future::join_all(futures).await;
                for (id, res) in results {
                    match res {
                        Ok(result) => {
                            // 回填子任务的文件变更与权限拒绝到父 run_state（FR-PERM-6）
                            run_state.file_changes.extend(result.file_changes.clone());
                            run_state
                                .permission_denials
                                .extend(result.permission_denials.clone());
                            let _ = event_tx
                                .send(AgentEvent::WorkerCompleted {
                                    task_id: result.task_id.clone(),
                                    status: result.status.clone(),
                                    summary: result.summary.clone(),
                                })
                                .await;
                            if result.status != TaskStatus::Success {
                                worker_success = false;
                            }
                            let content = format!(
                                "Task {} completed with status {:?}.\nSummary: {}",
                                result.task_id, result.status, result.summary
                            );
                            let is_error = result.status == TaskStatus::Failed;
                            let _ = event_tx
                                .send(AgentEvent::ToolResult {
                                    id: id.clone(),
                                    name: Some(DELEGATE_TASK_TOOL.to_string()),
                                    content: content.clone(),
                                    is_error,
                                    run_id: Some(runtime.run_id.clone()),
                                    turn_id: Some(turn_id.clone()),
                                    call_id: Some(call_id.clone()),
                                    duration_ms: None,
                                })
                                .await;
                            tool_results.push(ContentBlock::ToolResult {
                                tool_use_id: id,
                                content,
                                is_error,
                            });
                        }
                        Err(e) => {
                            worker_success = false;
                            let content = format!("Worker error: {e}");
                            let _ = event_tx
                                .send(AgentEvent::ToolResult {
                                    id: id.clone(),
                                    name: Some(DELEGATE_TASK_TOOL.to_string()),
                                    content: content.clone(),
                                    is_error: true,
                                    run_id: Some(runtime.run_id.clone()),
                                    turn_id: Some(turn_id.clone()),
                                    call_id: Some(call_id.clone()),
                                    duration_ms: None,
                                })
                                .await;
                            tool_results.push(ContentBlock::ToolResult {
                                tool_use_id: id,
                                content,
                                is_error: true,
                            });
                        }
                    }
                }
            } else {
                // Sequential: write workers, path conflicts, or single worker
                for (id, task) in tasks_parsed {
                    if cancel.is_cancelled() {
                        break;
                    }
                    let worker_tools: Vec<Arc<dyn Tool>> = self
                        .tools
                        .iter()
                        .filter(|t| task.tools_allowed.contains(&t.name().to_string()))
                        .cloned()
                        .collect();
                    let worker = WorkerAgent::new(
                        self.provider.clone(),
                        worker_tools,
                        self.model.clone(),
                        runtime.permission.clone(),
                        runtime.work_dir.clone(),
                    );
                    let _ = event_tx
                        .send(AgentEvent::WorkerSpawned {
                            task_id: task.id.clone(),
                            description: task.description.clone(),
                        })
                        .await;
                    match worker
                        .execute_task(&task, event_tx.clone(), cancel.clone())
                        .await
                    {
                        Ok(result) => {
                            // 回填子任务的文件变更与权限拒绝到父 run_state（FR-PERM-6）
                            run_state.file_changes.extend(result.file_changes.clone());
                            run_state
                                .permission_denials
                                .extend(result.permission_denials.clone());
                            let _ = event_tx
                                .send(AgentEvent::WorkerCompleted {
                                    task_id: result.task_id.clone(),
                                    status: result.status.clone(),
                                    summary: result.summary.clone(),
                                })
                                .await;
                            if result.status != TaskStatus::Success {
                                worker_success = false;
                            }
                            let content = format!(
                                "Task {} completed with status {:?}.\nSummary: {}",
                                result.task_id, result.status, result.summary
                            );
                            let is_error = result.status == TaskStatus::Failed;
                            let _ = event_tx
                                .send(AgentEvent::ToolResult {
                                    id: id.clone(),
                                    name: Some(DELEGATE_TASK_TOOL.to_string()),
                                    content: content.clone(),
                                    is_error,
                                    run_id: Some(runtime.run_id.clone()),
                                    turn_id: Some(turn_id.clone()),
                                    call_id: Some(call_id.clone()),
                                    duration_ms: None,
                                })
                                .await;
                            tool_results.push(ContentBlock::ToolResult {
                                tool_use_id: id,
                                content,
                                is_error,
                            });
                        }
                        Err(e) => {
                            worker_success = false;
                            let content = format!("Worker error: {e}");
                            let _ = event_tx
                                .send(AgentEvent::ToolResult {
                                    id: id.clone(),
                                    name: Some(DELEGATE_TASK_TOOL.to_string()),
                                    content: content.clone(),
                                    is_error: true,
                                    run_id: Some(runtime.run_id.clone()),
                                    turn_id: Some(turn_id.clone()),
                                    call_id: Some(call_id.clone()),
                                    duration_ms: None,
                                })
                                .await;
                            tool_results.push(ContentBlock::ToolResult {
                                tool_use_id: id,
                                content,
                                is_error: true,
                            });
                        }
                    }
                }
            }
        }

        // 追加 loop nudge 到 tool_results 同一消息，保持 tool_use/tool_result 配对
        if let Some(nudge) = pending_loop_nudge {
            tool_results.push(ContentBlock::Text { text: nudge });
        }
        let tool_results_msg = Message {
            role: Role::User,
            content: tool_results,
        };
        // 【SA 12】粗估增量：工具结果不等到下一次 LLM 调用才被预算察觉
        self.context_manager
            .add_pending(estimate_tokens(std::slice::from_ref(&tool_results_msg)));
        self.messages.push(tool_results_msg);

        // Budget check after tool results to catch large tool outputs early
        match self.context_manager.check_budget(&self.messages) {
            BudgetStatus::Overflow100 => {
                warn!("Budget overflow after tool results, terminating");
                let _ = event_tx
                    .send(AgentEvent::TurnCompleted {
                        run_id: runtime.run_id.clone(),
                        turn_id: turn_id.clone(),
                        timestamp: now_ts(),
                    })
                    .await;
                return Ok(ActResult::Done);
            }
            BudgetStatus::Critical95 => {
                let used = estimate_tokens(&self.messages);
                if let Some(_strategy) = self
                    .context_manager
                    .compress_async(&mut self.messages)
                    .await
                {
                    self.context_manager.reset_actual_tokens(); // FR-BUDGET-6: 防止旧值误判
                    let after = estimate_tokens(&self.messages);
                    let _ = event_tx
                        .send(AgentEvent::CompressionTriggered {
                            before_tokens: used,
                            after_tokens: after,
                            strategy: "post_tool_critical".to_string(),
                        })
                        .await;
                }
            }
            _ => {}
        }

        let _ = event_tx
            .send(AgentEvent::TurnCompleted {
                run_id: runtime.run_id.clone(),
                turn_id,
                timestamp: now_ts(),
            })
            .await;

        if had_worker {
            Ok(ActResult::WorkerCompleted {
                success: worker_success,
            })
        } else {
            Ok(ActResult::Continue)
        }
    }

    /// Execute a single tool with per-tool timeout and event emission.
    async fn execute_single_tool(
        &self,
        id: &str,
        name: &str,
        input: &serde_json::Value,
        event_tx: &mpsc::Sender<AgentEvent>,
        run_state: &mut RunState,
        runtime: &OrchestratorRuntime,
        turn_id: &str,
        call_id: &str,
    ) -> ContentBlock {
        let tool_runtime = ToolRuntime::new(
            runtime.permission.clone(),
            self.tools.clone(),
            runtime.work_dir.clone(),
        );
        tool_runtime
            .execute_tool_call(
                ToolCallContext {
                    id,
                    name,
                    input,
                    run_id: &runtime.run_id,
                    turn_id,
                    call_id: Some(call_id),
                },
                event_tx,
                run_state,
            )
            .await
    }

    /// Execute multiple read-only tools in parallel.
    async fn execute_tools_parallel(
        &self,
        tools: &[(&str, &str, &serde_json::Value)],
        event_tx: &mpsc::Sender<AgentEvent>,
        _cancel: &CancellationToken,
        run_state: &mut RunState,
        runtime: &OrchestratorRuntime,
        turn_id: &str,
        call_id: &str,
    ) -> Vec<ContentBlock> {
        use futures::future::join_all;

        let tool_runtime = ToolRuntime::new(
            runtime.permission.clone(),
            self.tools.clone(),
            runtime.work_dir.clone(),
        );
        let mut allowed = Vec::new();
        let mut content_blocks = Vec::new();
        for (id, name, input) in tools {
            match tool_runtime
                .gate_tool(
                    &ToolCallContext {
                        id: *id,
                        name: *name,
                        input: *input,
                        run_id: &runtime.run_id,
                        turn_id,
                        call_id: Some(call_id),
                    },
                    event_tx,
                    run_state,
                )
                .await
            {
                ToolGate::Proceed => allowed.push((*id, *name, *input)),
                ToolGate::Denied(reason) => {
                    let content = format!(
                        "Permission denied: {reason}. This action was not executed. If needed, the user can perform it manually."
                    );
                    let _ = event_tx
                        .send(AgentEvent::ToolResult {
                            id: (*id).to_string(),
                            name: Some((*name).to_string()),
                            content: content.clone(),
                            is_error: true,
                            run_id: Some(runtime.run_id.clone()),
                            turn_id: Some(turn_id.to_string()),
                            call_id: Some(call_id.to_string()),
                            duration_ms: Some(0),
                        })
                        .await;
                    content_blocks.push(ContentBlock::ToolResult {
                        tool_use_id: (*id).to_string(),
                        content,
                        is_error: true,
                    });
                }
            }
        }

        // Emit ToolStart events
        for (id, name, input) in &allowed {
            let input_str = serde_json::to_string(input).unwrap_or_default();
            let _ = event_tx
                .send(AgentEvent::ToolStart {
                    id: id.to_string(),
                    name: name.to_string(),
                    input: input_str,
                    run_id: Some(runtime.run_id.clone()),
                    turn_id: Some(turn_id.to_string()),
                    call_id: Some(call_id.to_string()),
                    risk: Some(format!(
                        "{:?}",
                        ToolRuntime::tool_risk_for(&self.tools, name)
                    )),
                    timeout_ms: Some(tool_runtime.timeout_for(name).as_millis() as u64),
                })
                .await;
        }

        // Execute all in parallel
        let futures: Vec<_> = allowed
            .iter()
            .map(|(id, name, input)| {
                let id = id.to_string();
                let name = name.to_string();
                let input = (*input).clone();
                let tool_runtime = tool_runtime.clone();
                let timeout = tool_runtime.timeout_for(&name);
                let event_tx = event_tx.clone();
                async move {
                    let start = Instant::now();
                    let output = match tokio::time::timeout(
                        timeout,
                        tool_runtime.execute_tool(&name, input, &event_tx, &id),
                    )
                    .await
                    {
                        Ok(tool_output) => tool_output,
                        Err(_) => ToolOutput {
                            content: format!(
                                "Tool execution timed out after {}s",
                                timeout.as_secs()
                            ),
                            is_error: true,
                        },
                    };
                    (id, name, output, start.elapsed().as_millis() as u64)
                }
            })
            .collect();

        let results = join_all(futures).await;

        for (id, name, output, duration_ms) in results {
            let _ = event_tx
                .send(AgentEvent::ToolResult {
                    id: id.clone(),
                    name: Some(name.clone()),
                    content: output.content.clone(),
                    is_error: output.is_error,
                    run_id: Some(runtime.run_id.clone()),
                    turn_id: Some(turn_id.to_string()),
                    call_id: Some(call_id.to_string()),
                    duration_ms: Some(duration_ms),
                })
                .await;
            let _ = event_tx
                .send(AgentEvent::ToolMetrics {
                    tool_name: name.clone(),
                    duration_ms,
                    is_error: output.is_error,
                })
                .await;
            let content = truncate_tool_result_default(&output.content);
            let content = if output.is_error {
                classify_tool_error(&content, &name)
            } else {
                content
            };
            content_blocks.push(ContentBlock::ToolResult {
                tool_use_id: id,
                content,
                is_error: output.is_error,
            });
        }

        content_blocks
    }
}

enum ActResult {
    Done,
    Continue,
    WorkerCompleted { success: bool },
}
