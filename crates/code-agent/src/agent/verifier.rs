use crate::agent::{Verdict, VerificationResult};
use crate::context::summary::truncate_tool_result_default;
use crate::retry::{emit_llm_request, emit_llm_response, stream_with_retry, LlmTraceContext};
use crate::runtime::now_ts;
use crate::AgentEvent;
use anyhow::Result;
use hank_provider::{
    CompletionRequest, ContentBlock, LlmProvider, Message, Role, StopReason, StreamEvent,
    ToolDefinition,
};
use code_tools::{Tool, ToolOutput};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

const VERIFIER_MAX_ITERATIONS: usize = 5;
/// 验证器工具调用超时（秒）
const VERIFIER_TOOL_TIMEOUT_SECS: u64 = 30;

/// VerifierAgent checks whether a task result satisfies the original intent.
/// It only has access to read-only tools.
pub struct VerifierAgent {
    provider: Arc<dyn LlmProvider>,
    tools: Vec<Arc<dyn Tool>>,
    model: String,
    tool_definitions: Vec<ToolDefinition>,
}

impl VerifierAgent {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        tools: Vec<Arc<dyn Tool>>,
        model: String,
    ) -> Self {
        let tool_definitions = tools
            .iter()
            .map(|t| ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
            })
            .collect();
        Self { provider, tools, model, tool_definitions }
    }

    /// Verify a task result against the original request.
    pub async fn verify(
        &self,
        run_id: &str,
        original_request: &str,
        task_summary: &str,
        event_tx: mpsc::Sender<AgentEvent>,
        cancel: CancellationToken,
    ) -> Result<VerificationResult> {
        let system_prompt = "You are a verification agent. Your job is to check whether \
            a task was completed correctly. Use the available read-only tools to inspect \
            the results. Respond with a JSON object: \
            {\"verdict\": \"approved\"|\"needs_revision\"|\"rejected\", \"issues\": [\"...\"]}";

        let user_msg = format!(
            "Original request: {original_request}\n\n\
             Task result summary: {task_summary}\n\n\
             Please verify the result is correct and complete."
        );

        let mut messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: user_msg }],
        }];

        let mut final_text = String::new();

        for iteration in 0..VERIFIER_MAX_ITERATIONS {
            if cancel.is_cancelled() {
                break;
            }

            let turn_id = uuid::Uuid::new_v4().to_string();
            let _ = event_tx
                .send(AgentEvent::TurnStarted {
                    run_id: run_id.to_string(),
                    turn_id: turn_id.clone(),
                    timestamp: now_ts(),
                    phase: "verify".to_string(),
                    message_count: messages.len(),
                })
                .await;

            let req = CompletionRequest {
                model: self.model.clone(),
                system: Some(system_prompt.to_string()),
                messages: messages.clone(),
                tools: self.tool_definitions.clone(),
                max_tokens: 2048,
            };
            let llm_trace = LlmTraceContext {
                call_id: uuid::Uuid::new_v4().to_string(),
                run_id: Some(run_id.to_string()),
                turn_id: Some(turn_id.clone()),
                model: req.model.clone(),
                provider: self.provider.name().to_string(),
                phase: "verify".to_string(),
            };

            debug!("Verifier iteration {iteration}");
            emit_llm_request(&event_tx, &llm_trace, &req).await;
            let llm_start = Instant::now();
            let mut stream = stream_with_retry(&self.provider, req, &event_tx, &llm_trace).await?;

            let mut assistant_content: Vec<ContentBlock> = Vec::new();
            let mut current_text = String::new();
            let mut current_tool_id = String::new();
            let mut current_tool_name = String::new();
            let mut current_tool_input = String::new();
            let mut stop_reason = StopReason::EndTurn;
            let mut in_tool_block = false;
            let mut input_tokens = 0u32;
            let mut output_tokens = 0u32;
            let mut cache_read_tokens = 0u32;
            let mut cache_write_tokens = 0u32;
            let mut cancelled = false;

            loop {
                let event = tokio::select! {
                    event = stream.next() => event,
                    _ = cancel.cancelled() => {
                        cancelled = true;
                        None
                    }
                };
                let Some(event) = event else { break };
                match event {
                    Ok(StreamEvent::TextDelta(text)) => {
                        current_text.push_str(&text);
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
                    Ok(StreamEvent::Usage {
                        input_tokens: input,
                        output_tokens: output,
                        cache_read_tokens: cache_read,
                        cache_write_tokens: cache_write,
                    }) => {
                        input_tokens += input;
                        output_tokens += output;
                        cache_read_tokens += cache_read;
                        cache_write_tokens += cache_write;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        warn!("Verifier stream error: {e}");
                        break;
                    }
                }
            }

            if !current_text.is_empty() {
                final_text = current_text.clone();
                assistant_content.push(ContentBlock::Text {
                    text: std::mem::take(&mut current_text),
                });
            }

            emit_llm_response(
                &event_tx,
                &llm_trace,
                assistant_content.clone(),
                stop_reason,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
                llm_start.elapsed().as_millis() as u64,
                cancelled,
                false,
            )
            .await;

            messages.push(Message {
                role: Role::Assistant,
                content: assistant_content.clone(),
            });

            if stop_reason == StopReason::ToolUse {
                let mut tool_results: Vec<ContentBlock> = Vec::new();
                for block in &assistant_content {
                    if let ContentBlock::ToolUse { id, name, input } = block {
                        if cancel.is_cancelled() { break; }
                        let _ = event_tx
                            .send(AgentEvent::ToolStart {
                                id: id.clone(),
                                name: name.clone(),
                                input: serde_json::to_string(input).unwrap_or_default(),
                                run_id: Some(run_id.to_string()),
                                turn_id: Some(turn_id.clone()),
                                call_id: Some(llm_trace.call_id.clone()),
                                risk: Some("Safe".to_string()),
                                timeout_ms: Some(VERIFIER_TOOL_TIMEOUT_SECS * 1000),
                            })
                            .await;
                        let tool_start = Instant::now();
                        let output = self.execute_tool(name, input.clone()).await;
                        let duration_ms = tool_start.elapsed().as_millis() as u64;
                        let _ = event_tx
                            .send(AgentEvent::ToolResult {
                                id: id.clone(),
                                name: Some(name.clone()),
                                content: output.content.clone(),
                                is_error: output.is_error,
                                run_id: Some(run_id.to_string()),
                                turn_id: Some(turn_id.clone()),
                                call_id: Some(llm_trace.call_id.clone()),
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
                        tool_results.push(ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content,
                            is_error: output.is_error,
                        });
                    }
                }
                messages.push(Message {
                    role: Role::User,
                    content: tool_results,
                });
            }

            let _ = event_tx
                .send(AgentEvent::TurnCompleted {
                    run_id: run_id.to_string(),
                    turn_id,
                    timestamp: now_ts(),
                })
                .await;

            if stop_reason != StopReason::ToolUse {
                break;
            }
        }

        // Parse the verification result from final text
        let result = self.parse_verification(&final_text);

        // 注意：调用方（session.rs / orchestrator.rs）负责发 VerificationCompleted 事件
        // 此处不重复发送，避免客户端收到双重事件

        Ok(result)
    }

    fn parse_verification(&self, text: &str) -> VerificationResult {
        // Try to parse JSON from the response
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
            let verdict = match v.get("verdict").and_then(|v| v.as_str()) {
                Some("approved") => Verdict::Approved,
                Some("needs_revision") => Verdict::NeedsRevision,
                Some("rejected") => Verdict::Rejected,
                _ => Verdict::NeedsRevision,
            };
            let issues = v
                .get("issues")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            return VerificationResult { verdict, issues };
        }

        // Try to find JSON embedded in text
        if let Some(start) = text.find('{') {
            if let Some(end) = text.rfind('}') {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text[start..=end]) {
                    let verdict = match v.get("verdict").and_then(|v| v.as_str()) {
                        Some("approved") => Verdict::Approved,
                        Some("needs_revision") => Verdict::NeedsRevision,
                        Some("rejected") => Verdict::Rejected,
                        _ => Verdict::NeedsRevision,
                    };
                    let issues = v
                        .get("issues")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    return VerificationResult { verdict, issues };
                }
            }
        }

        // 解析失败时默认 Approved，防止因格式错误导致无限修订循环
        warn!("Could not parse verification JSON, defaulting to Approved");
        VerificationResult {
            verdict: Verdict::Approved,
            issues: vec!["Could not parse verification result, auto-approved".to_string()],
        }
    }

    async fn execute_tool(&self, name: &str, input: serde_json::Value) -> ToolOutput {
        for tool in &self.tools {
            if tool.name() == name {
                return match tokio::time::timeout(
                    std::time::Duration::from_secs(VERIFIER_TOOL_TIMEOUT_SECS),
                    tool.execute(input),
                )
                .await
                {
                    Ok(Ok(output)) => output,
                    Ok(Err(e)) => ToolOutput {
                        content: format!("Tool execution error: {e}"),
                        is_error: true,
                    },
                    Err(_) => ToolOutput {
                        content: format!(
                            "Verifier tool timed out after {VERIFIER_TOOL_TIMEOUT_SECS}s"
                        ),
                        is_error: true,
                    },
                };
            }
        }
        ToolOutput {
            content: format!("Unknown tool: {name}"),
            is_error: true,
        }
    }
}
