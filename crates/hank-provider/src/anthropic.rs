use crate::{CompletionRequest, LlmProvider, StopReason, StreamEvent};
use anyhow::{bail, Result};
use async_trait::async_trait;
use futures::Stream;
use reqwest::Client;
use std::pin::Pin;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error, Instrument};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

pub struct AnthropicProvider {
    client: Client,
    api_key: String,
    base_url: String,
}

impl AnthropicProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }

    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url.trim_end_matches('/').to_string();
        self
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    async fn stream(
        &self,
        req: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let url = format!("{}/v1/messages", self.base_url);
        let body = build_request_body(&req);
        debug!(
            "Sending request to Anthropic API: {url}, model={}",
            req.model
        );

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                error!(provider = "anthropic", url = %url, error = %e, "Connection failed");
                anyhow::anyhow!("Failed to connect to {url}: {e}")
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            error!(provider = "anthropic", status = %status, url = %url, "API error");
            bail!("Anthropic API error {status}: {text}");
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
    use crate::ContentBlock;

    let mut messages: Vec<serde_json::Value> = Vec::new();
    for msg in &req.messages {
        let content: Vec<serde_json::Value> = msg
            .content
            .iter()
            .map(|block| match block {
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    let mut result = serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": [{"type": "text", "text": content}],
                    });
                    if *is_error {
                        result["is_error"] = serde_json::json!(true);
                    }
                    result
                }
                _ => serde_json::to_value(block).unwrap(),
            })
            .collect();
        messages.push(serde_json::json!({
            "role": msg.role,
            "content": content,
        }));
    }

    // 【SA 13】prompt cache breakpoint 2：最后一条 user 消息末尾（滚动前缀）
    if let Some(last_user) = messages
        .iter_mut()
        .rev()
        .find(|m| m["role"] == serde_json::json!("user"))
    {
        if let Some(content) = last_user["content"].as_array_mut() {
            if let Some(last_block) = content.last_mut() {
                last_block["cache_control"] = serde_json::json!({"type": "ephemeral"});
            }
        }
    }

    let mut body = serde_json::json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "messages": messages,
        "stream": true,
    });

    if let Some(system) = &req.system {
        // 【SA 13】prompt cache breakpoint 1：system 末尾（静态前缀，跨轮复用）
        body["system"] = serde_json::json!([{
            "type": "text",
            "text": system,
            "cache_control": {"type": "ephemeral"},
        }]);
    }

    if !req.tools.is_empty() {
        let tools: Vec<serde_json::Value> = req
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.input_schema,
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

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(pos) = buffer.find("\n\n") {
            let event_str = buffer[..pos].to_string();
            buffer.drain(..pos + 2);

            let events = parse_sse_event(&event_str);
            for event in events {
                debug!("Parsed StreamEvent: {event:?}");
                if tx.send(Ok(event)).await.is_err() {
                    return Ok(());
                }
            }
        }
    }

    Ok(())
}

fn parse_sse_event(raw: &str) -> Vec<StreamEvent> {
    let mut event_type = String::new();
    let mut data = String::new();

    for line in raw.lines() {
        if let Some(val) = line.strip_prefix("event: ") {
            event_type = val.to_string();
        } else if let Some(val) = line.strip_prefix("data: ") {
            data = val.to_string();
        }
    }

    if data.is_empty() {
        return Vec::new();
    }

    let parsed: serde_json::Value = match serde_json::from_str(&data) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    match event_type.as_str() {
        "message_start" => {
            if let Some(usage) = parsed.get("message").and_then(|m| m.get("usage")) {
                let input_tokens = usage
                    .get("input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                let output_tokens = usage
                    .get("output_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                // Anthropic 的 cache token 单列（不含在 input_tokens 内，不用减）
                let cache_read_tokens = usage
                    .get("cache_read_input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                let cache_write_tokens = usage
                    .get("cache_creation_input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                vec![StreamEvent::Usage {
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                }]
            } else {
                Vec::new()
            }
        }
        "content_block_start" => {
            let block = match parsed.get("content_block") {
                Some(b) => b,
                None => return Vec::new(),
            };
            let block_type = match block.get("type").and_then(|t| t.as_str()) {
                Some(t) => t,
                None => return Vec::new(),
            };
            if block_type == "tool_use" {
                let id = block
                    .get("id")
                    .and_then(|i| i.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                vec![StreamEvent::ToolUseStart { id, name }]
            } else {
                Vec::new()
            }
        }
        "content_block_delta" => {
            let delta = match parsed.get("delta") {
                Some(d) => d,
                None => return Vec::new(),
            };
            let delta_type = match delta.get("type").and_then(|t| t.as_str()) {
                Some(t) => t,
                None => return Vec::new(),
            };
            match delta_type {
                "text_delta" => {
                    if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                        vec![StreamEvent::TextDelta(text.to_string())]
                    } else {
                        Vec::new()
                    }
                }
                "input_json_delta" => {
                    if let Some(json) = delta.get("partial_json").and_then(|j| j.as_str()) {
                        vec![StreamEvent::ToolUseInputDelta(json.to_string())]
                    } else {
                        Vec::new()
                    }
                }
                _ => Vec::new(),
            }
        }
        "content_block_stop" => {
            vec![StreamEvent::ToolUseEnd]
        }
        "message_delta" => {
            let mut events = Vec::new();
            // Extract output_tokens from usage field
            if let Some(usage) = parsed.get("usage") {
                let output_tokens = usage
                    .get("output_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                if output_tokens > 0 {
                    events.push(StreamEvent::Usage {
                        input_tokens: 0,
                        output_tokens,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    });
                }
            }
            if let Some(delta) = parsed.get("delta") {
                if let Some(stop_reason) = delta.get("stop_reason").and_then(|s| s.as_str()) {
                    let reason = match stop_reason {
                        "end_turn" => StopReason::EndTurn,
                        "tool_use" => StopReason::ToolUse,
                        "max_tokens" => StopReason::MaxTokens,
                        _ => StopReason::EndTurn,
                    };
                    events.push(StreamEvent::MessageEnd {
                        stop_reason: reason,
                    });
                }
            }
            events
        }
        "error" => {
            let msg = parsed
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error")
                .to_string();
            vec![StreamEvent::Error(msg)]
        }
        _ => Vec::new(),
    }
}
