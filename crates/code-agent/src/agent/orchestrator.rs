use crate::agent::traits::{DelegatedTask, TaskStatus, ThinkStrategy};
use crate::agent::verifier::VerifierAgent;
use crate::agent::worker::WorkerAgent;
use crate::agent::LoopDetector;
use crate::context::summary::{estimate_tokens, truncate_tool_result_default};
use crate::context::{BudgetStatus, ContextManager};
use crate::retry::stream_with_retry;
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
const LOOP_TERMINATE_COUNT: usize = 3;
const MAX_REVISIONS: usize = 2;
/// LlmRequest 事件中 system prompt 最大字符数，防止 SSE 流量过大
const LLM_REQUEST_SYSTEM_PREVIEW_CHARS: usize = 500;

#[derive(Clone)]
pub(crate) struct OrchestratorRuntime {
    pub(crate) run_id: String,
    pub(crate) work_dir: String,
    pub(crate) permission: Arc<PermissionGuard>,
    pub(crate) verify_after_write: bool,
    pub(crate) original_request: String,
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
    consecutive_max_tokens: u32,
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
            consecutive_max_tokens: 0,
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

    /// LlmRequest 事件中截断 system prompt，防止 SSE 流量过大
    fn preview_system(system: &Option<String>) -> Option<String> {
        system.as_deref().map(|s| {
            match s.char_indices().nth(LLM_REQUEST_SYSTEM_PREVIEW_CHARS) {
                Some((i, _)) => format!("{}...", &s[..i]),
                None => s.to_string(),
            }
        })
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
        self.messages.push(Message {
            role: Role::User,
            content: user_content,
        });

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
                warn!("Orchestrator reached max iterations");
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

        let _ = event_tx
            .send(AgentEvent::LlmRequest {
                model: req.model.clone(),
                system: Self::preview_system(&req.system),
                tools: req.tools.iter().map(|t| t.name.clone()).collect(),
                max_tokens: req.max_tokens,
                message_count: req.messages.len(),
                phase: "think".to_string(),
            })
            .await;

        debug!("Orchestrator THINK phase");
        let llm_start = Instant::now();
        let mut stream = stream_with_retry(&self.provider, req).await?;
        let mut think_text = String::new();
        let mut total_input_tokens = 0u32;
        let mut total_output_tokens = 0u32;

        loop {
            let event = tokio::select! {
                event = stream.next() => event,
                _ = cancel.cancelled() => { None }
                _ = tokio::time::sleep(Duration::from_secs(LLM_STREAM_TIMEOUT_SECS)) => {
                    warn!("Think phase LLM stream timeout after {}s", LLM_STREAM_TIMEOUT_SECS);
                    None
                }
            };
            let Some(event) = event else { break };
            match event {
                Ok(StreamEvent::TextDelta(text)) => {
                    think_text.push_str(&text);
                    let _ = event_tx.send(AgentEvent::Thinking { text }).await;
                }
                Ok(StreamEvent::MessageEnd { .. }) => break,
                Ok(StreamEvent::Usage {
                    input_tokens,
                    output_tokens,
                }) => {
                    total_input_tokens += input_tokens;
                    total_output_tokens += output_tokens;
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

        // Add think output to messages as assistant turn
        if !think_text.is_empty() {
            self.messages.push(Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text { text: think_text }],
            });
        }

        let latency_ms = llm_start.elapsed().as_millis() as u64;
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
        run_state.input_tokens = run_state.input_tokens.max(total_input_tokens);
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
            max_tokens: 16384,
        };

        let _ = event_tx
            .send(AgentEvent::LlmRequest {
                model: req.model.clone(),
                system: Self::preview_system(&req.system),
                tools: req.tools.iter().map(|t| t.name.clone()).collect(),
                max_tokens: req.max_tokens,
                message_count: req.messages.len(),
                phase: "act".to_string(),
            })
            .await;

        debug!("Orchestrator ACT phase");
        let llm_start = Instant::now();
        let mut stream = stream_with_retry(&self.provider, req).await?;

        let mut assistant_content: Vec<ContentBlock> = Vec::new();
        let mut current_text = String::new();
        let mut current_tool_id = String::new();
        let mut current_tool_name = String::new();
        let mut current_tool_input = String::new();
        let mut stop_reason = StopReason::EndTurn;
        let mut in_tool_block = false;
        let mut total_input_tokens = 0u32;
        let mut total_output_tokens = 0u32;

        loop {
            let event = tokio::select! {
                event = stream.next() => event,
                _ = cancel.cancelled() => { None }
                _ = tokio::time::sleep(Duration::from_secs(LLM_STREAM_TIMEOUT_SECS)) => {
                    warn!("Act phase LLM stream timeout after {}s", LLM_STREAM_TIMEOUT_SECS);
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
                    error!("Act phase stream error: {e}");
                    return Err(e);
                }
            }
        }

        if !current_text.is_empty() {
            assistant_content.push(ContentBlock::Text {
                text: std::mem::take(&mut current_text),
            });
        }

        self.messages.push(Message {
            role: Role::Assistant,
            content: assistant_content.clone(),
        });

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
        run_state.input_tokens = run_state.input_tokens.max(total_input_tokens);
        run_state.output_tokens += total_output_tokens;
        if total_input_tokens > 0 {
            self.context_manager
                .update_actual_tokens(total_input_tokens as usize);
        }

        // Handle MaxTokens: continue generation instead of stopping
        if stop_reason == StopReason::MaxTokens {
            warn!("Orchestrator MaxTokens hit, continuing generation");
            self.consecutive_max_tokens += 1;
            if self.consecutive_max_tokens >= 3 {
                warn!("3 consecutive MaxTokens without tool use, treating as done");
                return Ok(ActResult::Done);
            }
            // Inject continuation prompt
            self.messages.push(Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "[Your previous response was cut off. Continue from where you left off.]"
                        .to_string(),
                }],
            });
            let _ = event_tx
                .send(AgentEvent::TurnCompleted {
                    run_id: runtime.run_id.clone(),
                    turn_id,
                    timestamp: now_ts(),
                })
                .await;
            return Ok(ActResult::Continue);
        }

        // Reset counter on successful tool use
        self.consecutive_max_tokens = 0;

        if stop_reason != StopReason::ToolUse {
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
                        self.messages.push(Message {
                            role: Role::User,
                            content: vec![ContentBlock::Text {
                                text: format!(
                                    "Verification issues found (revision {}/{MAX_REVISIONS}):\n{issue_text}\nPlease fix these issues.",
                                    self.verify_revision_count
                                ),
                            }],
                        });
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

        // FR-TOOL-7: 首次调用 deferred 工具时注入完整 schema（影响后续请求）
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
                }
            }
        }

        // Check for loops on regular tools
        for (id, name, input) in &regular_tools {
            if self.loop_detector.record_and_check(name, input) {
                let pattern = self.loop_detector.loop_pattern();
                let _ = event_tx
                    .send(AgentEvent::LoopDetected {
                        pattern: pattern.clone(),
                        window_size: 6,
                    })
                    .await;

                if self.loop_detector.should_terminate(LOOP_TERMINATE_COUNT) {
                    warn!("Loop detection: terminating after repeated loops");
                    tool_results.push(ContentBlock::ToolResult {
                        tool_use_id: id.to_string(),
                        content: format!(
                            "Loop detected: {}. Agent terminating to prevent infinite loop.",
                            pattern
                        ),
                        is_error: true,
                    });
                    // Add empty results for remaining tools
                    break;
                }

                // Inject nudge
                self.messages.push(Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text {
                        text: format!(
                            "⚠️ Loop detected: {}. Vary your approach or use different tools.",
                            pattern
                        ),
                    }],
                });
            }
        }

        // Execute regular tools — parallel if all are read-only, sequential otherwise
        if !regular_tools.is_empty() && tool_results.is_empty() {
            let has_writes = regular_tools
                .iter()
                .any(|(_, name, _)| self.tools.iter().any(|t| t.name() == *name && t.is_write()));

            if !has_writes && regular_tools.len() > 1 {
                // Parallel execution for read-only tools
                let results = self
                    .execute_tools_parallel(
                        &regular_tools,
                        &event_tx,
                        cancel,
                        run_state,
                        runtime,
                        &turn_id,
                    )
                    .await;
                tool_results.extend(results);
            } else {
                // Sequential execution for write tools
                for (id, name, input) in &regular_tools {
                    if cancel.is_cancelled() {
                        break;
                    }
                    let result = self
                        .execute_single_tool(
                            id, name, input, &event_tx, run_state, runtime, &turn_id,
                        )
                        .await;
                    tool_results.push(result);
                }
            }
        }

        // Execute delegate tasks — parallel if no affected_paths conflicts, sequential otherwise
        if !delegate_tasks.is_empty() {
            had_worker = true;

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
                            tool_results.push(ContentBlock::ToolResult {
                                tool_use_id: id,
                                content: format!(
                                    "Task {} completed with status {:?}.\nSummary: {}",
                                    result.task_id, result.status, result.summary
                                ),
                                is_error: result.status == TaskStatus::Failed,
                            });
                        }
                        Err(e) => {
                            worker_success = false;
                            tool_results.push(ContentBlock::ToolResult {
                                tool_use_id: id,
                                content: format!("Worker error: {e}"),
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
                            tool_results.push(ContentBlock::ToolResult {
                                tool_use_id: id,
                                content: format!(
                                    "Task {} completed with status {:?}.\nSummary: {}",
                                    result.task_id, result.status, result.summary
                                ),
                                is_error: result.status == TaskStatus::Failed,
                            });
                        }
                        Err(e) => {
                            worker_success = false;
                            tool_results.push(ContentBlock::ToolResult {
                                tool_use_id: id,
                                content: format!("Worker error: {e}"),
                                is_error: true,
                            });
                        }
                    }
                }
            }
        }

        self.messages.push(Message {
            role: Role::User,
            content: tool_results,
        });

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
                    },
                    event_tx,
                    run_state,
                )
                .await
            {
                ToolGate::Proceed => allowed.push((*id, *name, *input)),
                ToolGate::Denied(reason) => {
                    content_blocks.push(ContentBlock::ToolResult {
                        tool_use_id: (*id).to_string(),
                        content: format!(
                            "Permission denied: {reason}. This action was not executed. If needed, the user can perform it manually."
                        ),
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
                    content: output.content.clone(),
                    is_error: output.is_error,
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
