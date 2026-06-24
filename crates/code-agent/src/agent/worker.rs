use crate::agent::{Artifact, DelegatedTask, LoopDetector, TaskResult, TaskStatus};
use crate::context::{BudgetStatus, ContextManager};
use crate::retry::stream_with_retry;
use crate::runtime::{RunState, ToolCallContext, ToolRuntime};
use crate::AgentEvent;
use anyhow::Result;
use hank_provider::{
    CompletionRequest, ContentBlock, LlmProvider, Message, Role, StopReason, StreamEvent,
    ToolDefinition,
};
use code_tools::{PermissionGuard, Tool};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

const WORKER_MAX_ITERATIONS: usize = 25;
const LLM_STREAM_TIMEOUT_SECS: u64 = 120;
const LOOP_TERMINATE_COUNT: usize = 3;
/// Worker context budget (smaller than orchestrator)
const WORKER_CONTEXT_BUDGET: usize = 100_000;
const WORKER_COMPRESS_THRESHOLD: usize = 60_000;

/// WorkerAgent executes a delegated task using a flat stream-tools loop.
/// 工具执行经 ToolRuntime 统一门控（权限/sandbox/超时/文件变更追踪），
/// 不再绕过 PermissionGuard（FR-PERM-6）。
pub struct WorkerAgent {
    provider: Arc<dyn LlmProvider>,
    model: String,
    tool_definitions: Vec<ToolDefinition>,
    context_manager: ContextManager,
    /// 工具执行运行时（继承父 Agent 的权限与工作目录）
    tool_runtime: ToolRuntime,
}

impl WorkerAgent {
    /// 创建 Worker。permission/work_dir 由 Orchestrator 透传，确保子任务
    /// 的工具调用与父 Agent 受同一权限边界约束。
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        tools: Vec<Arc<dyn Tool>>,
        model: String,
        permission: Arc<PermissionGuard>,
        work_dir: impl Into<String>,
    ) -> Self {
        let tool_definitions = tools
            .iter()
            .map(|t| ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
            })
            .collect();
        let context_manager = ContextManager::with_budget(
            WORKER_COMPRESS_THRESHOLD,
            WORKER_CONTEXT_BUDGET,
            provider.clone(),
            model.clone(),
        );
        let work_dir = work_dir.into();
        let tool_runtime = ToolRuntime::new(permission, tools.clone(), work_dir);
        Self {
            provider,
            model,
            tool_definitions,
            context_manager,
            tool_runtime,
        }
    }

    /// Execute a delegated task and return a TaskResult.
    pub async fn execute_task(
        &self,
        task: &DelegatedTask,
        event_tx: mpsc::Sender<AgentEvent>,
        cancel: CancellationToken,
    ) -> Result<TaskResult> {
        let system_prompt = format!(
            "You are a worker agent executing a specific sub-task.\n\
             Task: {}\n\
             Context: {}\n\
             Complete this task thoroughly. Report your findings clearly.",
            task.description, task.context
        );

        let mut messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: task.description.clone(),
            }],
        }];

        let mut artifacts = Vec::new();
        let mut final_text = String::new();
        let mut consecutive_max_tokens = 0u32;
        let mut loop_detector = LoopDetector::new();
        // 本地 run_state：收集子任务的文件变更与权限拒绝，结束后回填父 run_state
        let mut run_state = RunState {
            run_id: task.id.clone(),
            ..Default::default()
        };

        for iteration in 0..WORKER_MAX_ITERATIONS {
            if cancel.is_cancelled() {
                break;
            }

            let req = CompletionRequest {
                model: self.model.clone(),
                system: Some(system_prompt.clone()),
                messages: messages.clone(),
                tools: self.tool_definitions.clone(),
                max_tokens: 8192,
            };

            debug!("Worker iteration {iteration} for task {}", task.id);

            let _ = event_tx.send(AgentEvent::LlmRequest {
                model: req.model.clone(),
                system: req.system.clone(),
                tools: req.tools.iter().map(|t| t.name.clone()).collect(),
                max_tokens: req.max_tokens,
                message_count: req.messages.len(),
                phase: "worker".to_string(),
            }).await;

            let mut stream = stream_with_retry(&self.provider, req).await?;
            let mut assistant_content: Vec<ContentBlock> = Vec::new();
            let mut current_text = String::new();
            let mut current_tool_id = String::new();
            let mut current_tool_name = String::new();
            let mut current_tool_input = String::new();
            let mut stop_reason = StopReason::EndTurn;
            let mut in_tool_block = false;
            let mut cancelled = false;

            loop {
                let event = tokio::select! {
                    event = stream.next() => event,
                    _ = cancel.cancelled() => { cancelled = true; None }
                    _ = tokio::time::sleep(Duration::from_secs(LLM_STREAM_TIMEOUT_SECS)) => {
                        warn!("Worker LLM stream timeout after {}s for task {}", LLM_STREAM_TIMEOUT_SECS, task.id);
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
                        if !in_tool_block { continue; }
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
                    Ok(StreamEvent::Error(msg)) => {
                        let _ = event_tx.send(AgentEvent::Error { message: msg }).await;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        return Ok(TaskResult {
                            task_id: task.id.clone(),
                            status: TaskStatus::Failed,
                            summary: format!("Stream error: {e}"),
                            artifacts: vec![],
                            file_changes: run_state.file_changes.clone(),
                            permission_denials: run_state.permission_denials.clone(),
                        });
                    }
                }
            }

            if !current_text.is_empty() {
                assistant_content.push(ContentBlock::Text {
                    text: std::mem::take(&mut current_text),
                });
            }

            // Collect final text for summary
            for block in &assistant_content {
                if let ContentBlock::Text { text } = block {
                    final_text = text.clone();
                }
            }

            messages.push(Message {
                role: Role::Assistant,
                content: assistant_content.clone(),
            });

            if cancelled {
                break;
            }

            // Handle MaxTokens: continue generation instead of stopping
            if stop_reason == StopReason::MaxTokens {
                warn!("Worker MaxTokens hit at iteration {iteration} for task {}", task.id);
                consecutive_max_tokens += 1;
                if consecutive_max_tokens >= 3 {
                    warn!("Worker: 3 consecutive MaxTokens without tool use, treating as done");
                    break;
                }
                // Inject continuation prompt
                messages.push(Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text {
                        text: "[Your previous response was cut off. Continue from where you left off.]".to_string(),
                    }],
                });
                continue;
            }

            if stop_reason == StopReason::ToolUse {
                consecutive_max_tokens = 0;
                let mut tool_results: Vec<ContentBlock> = Vec::new();
                for block in &assistant_content {
                    if let ContentBlock::ToolUse { id, name, input } = block {
                        if cancel.is_cancelled() {
                            return Ok(TaskResult {
                                task_id: task.id.clone(),
                                status: TaskStatus::Failed,
                                summary: "Cancelled".to_string(),
                                artifacts: vec![],
                                file_changes: run_state.file_changes.clone(),
                                permission_denials: run_state.permission_denials.clone(),
                            });
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

                            if loop_detector.should_terminate(LOOP_TERMINATE_COUNT) {
                                warn!(
                                    "Worker loop detection: terminating task {}",
                                    task.id
                                );
                                return Ok(TaskResult {
                                    task_id: task.id.clone(),
                                    status: TaskStatus::Failed,
                                    summary: format!("Loop detected: {}. Task terminated.", pattern),
                                    artifacts,
                                    file_changes: run_state.file_changes.clone(),
                                    permission_denials: run_state.permission_denials.clone(),
                                });
                            }

                            // Inject nudge message
                            messages.push(Message {
                                role: Role::User,
                                content: vec![ContentBlock::Text {
                                    text: format!(
                                        "⚠️ Loop detected: {}. Vary your approach or use different tools.",
                                        pattern
                                    ),
                                }],
                            });
                        }

                        // 经 ToolRuntime 统一执行：权限门控 + sandbox + 超时 +
                        // 文件变更追踪 + 截断 + 错误分类（FR-PERM-6）
                        let result_block = self
                            .tool_runtime
                            .execute_tool_call(
                                ToolCallContext {
                                    id: id.as_str(),
                                    name: name.as_str(),
                                    input,
                                    run_id: &task.id,
                                    turn_id: &task.id,
                                },
                                &event_tx,
                                &mut run_state,
                            )
                            .await;

                        // 收集成功工具输出为 artifact（供 orchestrator 汇总）
                        if let ContentBlock::ToolResult { content, is_error, .. } = &result_block {
                            if !*is_error {
                                artifacts.push(Artifact {
                                    kind: name.clone(),
                                    description: format!("Output from {name}"),
                                    content: content.clone(),
                                });
                            }
                        }
                        tool_results.push(result_block);
                    }
                }
                messages.push(Message {
                    role: Role::User,
                    content: tool_results,
                });

                // Budget check after tool results
                match self.context_manager.check_budget(&messages) {
                    BudgetStatus::Overflow100 => {
                        warn!("Worker budget overflow, terminating task {}", task.id);
                        break;
                    }
                    BudgetStatus::Critical95 | BudgetStatus::Warning80 => {
                        if self.context_manager.needs_compression(&messages) {
                            self.context_manager.compress_async(&mut messages).await;
                        }
                    }
                    BudgetStatus::Normal => {}
                }
            } else {
                break;
            }

            if iteration == WORKER_MAX_ITERATIONS - 1 {
                warn!("Worker reached max iterations for task {}", task.id);
            }
        }

        // Truncate summary to reasonable length（按字符边界，避免多字节字符切片 panic）
        let summary = if final_text.chars().count() > 500 {
            let end = final_text
                .char_indices()
                .nth(500)
                .map(|(i, _)| i)
                .unwrap_or(final_text.len());
            format!("{}...", &final_text[..end])
        } else if final_text.is_empty() {
            "Task completed without output.".to_string()
        } else {
            final_text
        };

        // Determine final status based on how we exited
        let status = if cancel.is_cancelled() {
            TaskStatus::Failed
        } else {
            TaskStatus::Success
        };

        Ok(TaskResult {
            task_id: task.id.clone(),
            status,
            summary,
            artifacts,
            file_changes: run_state.file_changes,
            permission_denials: run_state.permission_denials,
        })
    }
}
