use crate::{CompletionRequest, LlmProvider, StopReason, StreamEvent};
use anyhow::{bail, Result};
use async_trait::async_trait;
use futures::Stream;
use reqwest::Client;
use std::pin::Pin;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error, Instrument};

const DEFAULT_BASE_URL: &str = "https://api.openai.com";

pub struct OpenAiProvider {
    client: Client,
    api_key: String,
    base_url: String,
    name: String,
}

impl OpenAiProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            base_url: DEFAULT_BASE_URL.to_string(),
            name: "openai".to_string(),
        }
    }

    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url.trim_end_matches('/').to_string();
        self
    }

    pub fn with_name(mut self, name: String) -> Self {
        self.name = name;
        self
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn stream(
        &self,
        req: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let url = format!("{}/v1/chat/completions", self.base_url);
        let body = build_request_body(&req);
        debug!("Sending request to OpenAI-compatible API: {url}");

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                error!(provider = %self.name, url = %url, error = %e, "Connection failed");
                anyhow::anyhow!("Failed to connect to {url}: {e}")
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            error!(provider = %self.name, status = %status, url = %url, "API error");
            bail!("OpenAI API error {status}: {text}");
        }

        let (tx, rx) = mpsc::channel(1);
        let byte_stream = response.bytes_stream();

        tokio::spawn(
            async move {
                if let Err(e) = process_sse_stream(byte_stream, &tx).await {
                    let _ = tx.send(Err(e)).await;
                }
            }
            .instrument(tracing::Span::current()),
        );

        Ok(Box::pin(ReceiverStream::new(rx)))
    }
}

fn build_request_body(req: &CompletionRequest) -> serde_json::Value {
    let mut messages: Vec<serde_json::Value> = Vec::new();

    // System message
    if let Some(system) = &req.system {
        messages.push(serde_json::json!({
            "role": "system",
            "content": system,
        }));
    }

    // Convert our messages to OpenAI format
    for msg in &req.messages {
        let role = match msg.role {
            crate::Role::User => "user",
            crate::Role::Assistant => "assistant",
        };

        let mut parts: Vec<serde_json::Value> = Vec::new();
        let mut tool_calls: Vec<serde_json::Value> = Vec::new();
        let mut tool_results: Vec<serde_json::Value> = Vec::new();

        for block in &msg.content {
            match block {
                crate::ContentBlock::Text { text } => {
                    parts.push(serde_json::json!({"type": "text", "text": text}));
                }
                crate::ContentBlock::Image { source } => {
                    let data_url = format!("data:{};base64,{}", source.media_type, source.data);
                    parts.push(serde_json::json!({
                        "type": "image_url",
                        "image_url": { "url": data_url }
                    }));
                }
                crate::ContentBlock::ToolUse { id, name, input } => {
                    tool_calls.push(serde_json::json!({
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": serde_json::to_string(input).unwrap_or_default(),
                        }
                    }));
                }
                crate::ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => {
                    tool_results.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": tool_use_id,
                        "content": content,
                    }));
                }
            }
        }

        if !tool_calls.is_empty() {
            // Assistant message with tool calls
            let text_content = parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("");
            let mut msg_obj = serde_json::json!({
                "role": "assistant",
                "tool_calls": tool_calls,
            });
            if !text_content.is_empty() {
                msg_obj["content"] = serde_json::json!(text_content);
            }
            messages.push(msg_obj);
        } else if !tool_results.is_empty() {
            // Tool result messages
            for tr in tool_results {
                messages.push(tr);
            }
        } else if parts.len() == 1 {
            // Simple text message
            if let Some(text) = parts[0].get("text") {
                messages.push(serde_json::json!({
                    "role": role,
                    "content": text,
                }));
            }
        } else if !parts.is_empty() {
            messages.push(serde_json::json!({
                "role": role,
                "content": parts,
            }));
        }
    }

    let mut body = serde_json::json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "messages": messages,
        "stream": true,
        "stream_options": {"include_usage": true},
    });

    if !req.tools.is_empty() {
        let tools: Vec<serde_json::Value> = req
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    }
                })
            })
            .collect();
        body["tools"] = serde_json::json!(tools);
    }

    body
}

async fn process_sse_stream(
    mut stream: impl futures::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
    tx: &mpsc::Sender<Result<StreamEvent>>,
) -> Result<()> {
    use futures::StreamExt;

    let mut buffer = String::new();
    let mut current_tool_id = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(pos) = buffer.find("\n\n") {
            let event_str = buffer[..pos].to_string();
            buffer.drain(..pos + 2);

            for line in event_str.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        return Ok(());
                    }

                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                        if let Some(events) = parse_chunk(&parsed, &mut current_tool_id) {
                            for event in events {
                                if tx.send(Ok(event)).await.is_err() {
                                    return Ok(());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

fn parse_chunk(
    chunk: &serde_json::Value,
    current_tool_id: &mut String,
) -> Option<Vec<StreamEvent>> {
    let mut events = Vec::new();

    // OpenAI sends usage in a final chunk with empty choices
    if let Some(usage) = chunk.get("usage") {
        let prompt_tokens = usage
            .get("prompt_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let output_tokens = usage
            .get("completion_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        // OpenAI 的 cached 已含在 prompt_tokens 里，归一化时从 input 扣除（【SA 13】）
        let cache_read_tokens = usage
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let input_tokens = prompt_tokens.saturating_sub(cache_read_tokens);
        events.push(StreamEvent::Usage {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens: 0,
        });
    }

    let choices = chunk.get("choices")?.as_array()?;
    if choices.is_empty() {
        return if events.is_empty() {
            None
        } else {
            Some(events)
        };
    }
    let choice = choices.first()?;
    let delta = choice.get("delta")?;
    let finish_reason = choice.get("finish_reason").and_then(|v| v.as_str());

    // Text content
    if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
        if !content.is_empty() {
            events.push(StreamEvent::TextDelta(content.to_string()));
        }
    }

    // Tool calls
    if let Some(tool_calls) = delta.get("tool_calls").and_then(|tc| tc.as_array()) {
        for tc in tool_calls {
            if let Some(function) = tc.get("function") {
                // New tool call start
                if let Some(name) = function.get("name").and_then(|n| n.as_str()) {
                    // Close previous tool call if one was in progress
                    if !current_tool_id.is_empty() {
                        events.push(StreamEvent::ToolUseEnd);
                    }
                    let id = tc
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    *current_tool_id = id.clone();
                    events.push(StreamEvent::ToolUseStart {
                        id,
                        name: name.to_string(),
                    });
                }
                // Arguments delta
                if let Some(args) = function.get("arguments").and_then(|a| a.as_str()) {
                    if !args.is_empty() {
                        events.push(StreamEvent::ToolUseInputDelta(args.to_string()));
                    }
                }
            }
        }
    }

    // Finish reason
    if let Some(reason) = finish_reason {
        match reason {
            "tool_calls" => {
                // Close the last tool call
                if !current_tool_id.is_empty() {
                    events.push(StreamEvent::ToolUseEnd);
                    current_tool_id.clear();
                }
                events.push(StreamEvent::MessageEnd {
                    stop_reason: StopReason::ToolUse,
                });
            }
            "stop" => {
                events.push(StreamEvent::MessageEnd {
                    stop_reason: StopReason::EndTurn,
                });
            }
            "length" => {
                events.push(StreamEvent::MessageEnd {
                    stop_reason: StopReason::MaxTokens,
                });
            }
            _ => {}
        }
    }

    if events.is_empty() {
        None
    } else {
        Some(events)
    }
}
