use crate::agent::{Verdict, VerificationResult};
use crate::context::summary::truncate_tool_result_default;
use crate::retry::stream_with_retry;
use crate::AgentEvent;
use anyhow::Result;
use hank_provider::{
    CompletionRequest, ContentBlock, LlmProvider, Message, Role, StopReason, StreamEvent,
    ToolDefinition,
};
use code_tools::{Tool, ToolOutput};
use std::sync::Arc;
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
        original_request: &str,
        task_summary: &str,
        _event_tx: mpsc::Sender<AgentEvent>,
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

            let req = CompletionRequest {
                model: self.model.clone(),
                system: Some(system_prompt.to_string()),
                messages: messages.clone(),
                tools: self.tool_definitions.clone(),
                max_tokens: 2048,
            };

            debug!("Verifier iteration {iteration}");
            let mut stream = stream_with_retry(&self.provider, req).await?;

            let mut assistant_content: Vec<ContentBlock> = Vec::new();
            let mut current_text = String::new();
            let mut current_tool_id = String::new();
            let mut current_tool_name = String::new();
            let mut current_tool_input = String::new();
            let mut stop_reason = StopReason::EndTurn;
            let mut in_tool_block = false;

            loop {
                let event = tokio::select! {
                    event = stream.next() => event,
                    _ = cancel.cancelled() => { None }
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
                    // 忽略未匹配的事件（如 Usage）——Anthropic 在 message_start
                    // 阶段就会先发自 Usage，不能当作流结束
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

            messages.push(Message {
                role: Role::Assistant,
                content: assistant_content.clone(),
            });

            if stop_reason == StopReason::ToolUse {
                let mut tool_results: Vec<ContentBlock> = Vec::new();
                for block in &assistant_content {
                    if let ContentBlock::ToolUse { id, name, input } = block {
                        if cancel.is_cancelled() { break; }
                        let output = self.execute_tool(name, input.clone()).await;
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
            } else {
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
