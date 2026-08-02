use crate::agent::{DelegatedTask, LoopDetector, LoopLevel, TaskResult, TaskStatus};
use crate::context::summary::estimate_tokens;
use crate::context::{BudgetStatus, ContextManager};
use crate::retry::{consume_stream_with_retry, LlmTraceContext};
use crate::runtime::{RunState, ToolCallContext, ToolRuntime};
use crate::AgentEvent;
use anyhow::Result;
use code_tools::{PermissionGuard, Tool};
use hank_provider::{
    CompletionRequest, ContentBlock, LlmProvider, Message, Role, StopReason, ToolDefinition,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

const WORKER_MAX_ITERATIONS: usize = 25;
const LLM_STREAM_TIMEOUT_SECS: u64 = 120;
/// MaxTokens 首次命中时静默提高的输出上限（【AF 08】8K→64K）
const MAX_OUTPUT_TOKENS_UPPER: u32 = 64_000;
/// 续写恢复消息最大注入次数（【AF 08】最多 3 次）
const MAX_CONTINUATIONS: u32 = 3;
/// 递减回报判停：续写增量低于该 token 数视为无进展（【AF 08】<500）
const CONTINUATION_MIN_GAIN_TOKENS: u32 = 500;
/// Worker context budget (smaller than orchestrator)
const WORKER_CONTEXT_BUDGET: usize = 100_000;
const WORKER_COMPRESS_THRESHOLD: usize = 60_000;

/// Worker 循环退出原因（【AF 08】退出必须带原因；【SA 22】异常退出标 [partial result]）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerExit {
    /// 模型正常说完话（EndTurn）
    Completed,
    /// 达到 WORKER_MAX_ITERATIONS
    MaxIterations,
    /// 上下文预算溢出
    BudgetOverflow,
    /// LLM 流超时
    StreamTimeout,
    /// 被取消
    Cancelled,
}

impl WorkerExit {
    fn reason(&self) -> &'static str {
        match self {
            WorkerExit::Completed => "completed",
            WorkerExit::MaxIterations => "reached max iterations",
            WorkerExit::BudgetOverflow => "context budget overflow",
            WorkerExit::StreamTimeout => "LLM stream timeout",
            WorkerExit::Cancelled => "cancelled",
        }
    }
}

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

        let user_msg = Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: task.description.clone(),
            }],
        };
        // 【SA 12】粗估增量：push 新消息后累加 pending，预算检查用 actual + pending
        self.context_manager
            .add_pending(estimate_tokens(std::slice::from_ref(&user_msg)));
        let mut messages = vec![user_msg];

        let mut final_text = String::new();
        // MaxTokens 恢复状态（【AF 08】三步递进 + 递减回报检测）
        let mut max_tokens_escalated = false;
        let mut continuation_count = 0u32;
        let mut small_output_streak = 0u32;
        let mut loop_detector = LoopDetector::new();
        // 循环退出原因（默认 MaxIterations：for 循环耗尽时保持该值）
        let mut exit_reason = WorkerExit::MaxIterations;
        // 本地 run_state：收集子任务的文件变更与权限拒绝，结束后回填父 run_state
        let mut run_state = RunState {
            run_id: task.id.clone(),
            ..Default::default()
        };

        for iteration in 0..WORKER_MAX_ITERATIONS {
            if cancel.is_cancelled() {
                exit_reason = WorkerExit::Cancelled;
                break;
            }

            let req = CompletionRequest {
                model: self.model.clone(),
                system: Some(system_prompt.clone()),
                messages: messages.clone(),
                tools: self.tool_definitions.clone(),
                max_tokens: if max_tokens_escalated {
                    MAX_OUTPUT_TOKENS_UPPER
                } else {
                    8192
                },
            };

            debug!("Worker iteration {iteration} for task {}", task.id);
            let turn_id = format!("{}:{iteration}", task.id);
            let call_id = uuid::Uuid::new_v4().to_string();
            let llm_trace = LlmTraceContext {
                call_id: call_id.clone(),
                run_id: Some(task.id.clone()),
                turn_id: Some(turn_id.clone()),
                model: req.model.clone(),
                provider: self.provider.name().to_string(),
                phase: "worker".to_string(),
            };

            // 步骤级重试：发请求 + 消费流作为一个可重试单元（【SA 03】）
            let outcome = match consume_stream_with_retry(
                &self.provider,
                req,
                &event_tx,
                &cancel,
                Duration::from_secs(LLM_STREAM_TIMEOUT_SECS),
                &llm_trace,
            )
            .await
            {
                Ok(outcome) => outcome,
                Err(e) => {
                    return Ok(TaskResult {
                        task_id: task.id.clone(),
                        status: TaskStatus::Failed,
                        summary: format!("Stream error: {e}"),
                        file_changes: run_state.file_changes.clone(),
                        permission_denials: run_state.permission_denials.clone(),
                    });
                }
            };
            let assistant_content = outcome.assistant_content;
            let stop_reason = outcome.stop_reason;
            let cancelled = outcome.cancelled;

            // 超时是显式失败：终止任务，不能落入 EndTurn 正常收尾路径（【SA 03】）
            if outcome.timed_out {
                warn!(
                    "Worker LLM stream timed out after {}s for task {}",
                    LLM_STREAM_TIMEOUT_SECS, task.id
                );
                exit_reason = WorkerExit::StreamTimeout;
                break;
            }

            // Collect final text for summary
            for block in &assistant_content {
                if let ContentBlock::Text { text } = block {
                    final_text = text.clone();
                }
            }

            let assistant_msg = Message {
                role: Role::Assistant,
                content: assistant_content.clone(),
            };
            self.context_manager
                .add_pending(estimate_tokens(std::slice::from_ref(&assistant_msg)));
            messages.push(assistant_msg);

            if cancelled {
                exit_reason = WorkerExit::Cancelled;
                break;
            }

            // Handle MaxTokens: continue generation instead of stopping.
            // 方案 A（【AF 07】）：若截断前已有完整 tool_use 块，照常执行这些工具、
            // push tool_results 后继续循环，保持配对，不注入续写提示。
            let max_tokens_with_tools = stop_reason == StopReason::MaxTokens
                && assistant_content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolUse { .. }));
            if stop_reason == StopReason::MaxTokens && !max_tokens_with_tools {
                // 【AF 08】① 首次命中：静默提高 max_output_tokens 重试，不注入消息
                if !max_tokens_escalated {
                    warn!("Worker MaxTokens hit at iteration {iteration} for task {}, escalating max_tokens to {MAX_OUTPUT_TOKENS_UPPER} and retrying", task.id);
                    max_tokens_escalated = true;
                    continue;
                }
                continuation_count += 1;
                // 递减回报检测：续写 ≥3 次且连续 2 次增量 <500 token → 直接停止
                if outcome.output_tokens < CONTINUATION_MIN_GAIN_TOKENS {
                    small_output_streak += 1;
                } else {
                    small_output_streak = 0;
                }
                if continuation_count > MAX_CONTINUATIONS
                    || (continuation_count >= MAX_CONTINUATIONS && small_output_streak >= 2)
                {
                    // ③ 认栽：发事件告知输出被截断
                    warn!(
                        "Worker MaxTokens recovery exhausted for task {}, output truncated",
                        task.id
                    );
                    let _ = event_tx
                        .send(AgentEvent::Error {
                            message: "Output truncated: the response hit the output token limit repeatedly and could not be completed. The partial output above is kept as-is.".to_string(),
                        })
                        .await;
                    exit_reason = WorkerExit::Completed;
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
                messages.push(continuation_msg);
                continue;
            }

            if stop_reason == StopReason::ToolUse || max_tokens_with_tools {
                continuation_count = 0;
                small_output_streak = 0;
                let mut tool_results: Vec<ContentBlock> = Vec::new();
                let mut pending_loop_nudge: Option<String> = None;
                for block in &assistant_content {
                    if let ContentBlock::ToolUse { id, name, input } = block {
                        if cancel.is_cancelled() {
                            return Ok(TaskResult {
                                task_id: task.id.clone(),
                                status: TaskStatus::Failed,
                                summary: "Cancelled".to_string(),
                                file_changes: run_state.file_changes.clone(),
                                permission_denials: run_state.permission_denials.clone(),
                            });
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
                                warn!("Worker loop detection: terminating task {}", task.id);
                                // 终止前给本条 assistant 消息中所有 tool_use 补全
                                // tool_result（含当前及未执行的），保持配对，避免
                                // 上层持久化 messages 再恢复时下一轮请求 400
                                tool_results.push(ContentBlock::ToolResult {
                                    tool_use_id: id.clone(),
                                    content: format!(
                                        "Loop detected: {}. Task terminated.",
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
                                                content: "Loop detected, execution aborted"
                                                    .to_string(),
                                                is_error: true,
                                            });
                                        }
                                    }
                                }
                                let abort_msg = Message {
                                    role: Role::User,
                                    content: tool_results,
                                };
                                self.context_manager
                                    .add_pending(estimate_tokens(std::slice::from_ref(&abort_msg)));
                                messages.push(abort_msg);
                                return Ok(TaskResult {
                                    task_id: task.id.clone(),
                                    status: TaskStatus::Failed,
                                    summary: format!(
                                        "Loop detected: {}. Task terminated.",
                                        pattern
                                    ),
                                    file_changes: run_state.file_changes.clone(),
                                    permission_denials: run_state.permission_denials.clone(),
                                });
                            }

                            // 未达熔断阈值（warning/critical）：记录 nudge，稍后追加到
                            // tool_results 消息末尾。不单独 push user 消息，以免插在
                            // assistant(tool_use) 与 tool_results 之间破坏配对。
                            pending_loop_nudge = Some(format!(
                                "⚠️ Loop detected: {}. Vary your approach or use different tools.",
                                pattern
                            ));
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
                                    turn_id: &turn_id,
                                    call_id: Some(&call_id),
                                },
                                &event_tx,
                                &mut run_state,
                            )
                            .await;

                        // 回填结果指纹（【SA 03】：结果不同属正常探索，不计 streak）
                        if let ContentBlock::ToolResult { content, .. } = &result_block {
                            loop_detector.record_result(name, input, content);
                        }

                        // P3-#19：不再把每个工具输出 clone 进 artifacts——
                        // orchestrator 只消费 summary，全量 clone（截断后仍可达
                        // 40K 字符/次）纯属浪费；结构化产出由 file_changes 回填。
                        tool_results.push(result_block);
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
                messages.push(tool_results_msg);

                // Budget check after tool results
                match self.context_manager.check_budget(&messages) {
                    BudgetStatus::Overflow100 => {
                        warn!("Worker budget overflow, terminating task {}", task.id);
                        exit_reason = WorkerExit::BudgetOverflow;
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
                exit_reason = WorkerExit::Completed;
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

        // #15：达到迭代上限时发明确事件告知（【AF 08】停了、为什么停）
        if exit_reason == WorkerExit::MaxIterations {
            let _ = event_tx
                .send(AgentEvent::Error {
                    message: format!(
                        "Worker task {} stopped: reached max iterations ({WORKER_MAX_ITERATIONS}). Partial result is returned; consider breaking the task into smaller steps.",
                        task.id
                    ),
                })
                .await;
        }

        // Determine final status based on exit reason（【SA 22】异常退出标 [partial result]）
        let status = match exit_reason {
            WorkerExit::Completed => TaskStatus::Success,
            _ => TaskStatus::Failed,
        };
        let summary = match exit_reason {
            WorkerExit::Completed => summary,
            _ => format!("[partial result: {}] {summary}", exit_reason.reason()),
        };

        Ok(TaskResult {
            task_id: task.id.clone(),
            status,
            summary,
            file_changes: run_state.file_changes,
            permission_denials: run_state.permission_denials,
        })
    }
}
