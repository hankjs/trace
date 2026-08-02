use crate::AgentEvent;
use anyhow::Result;
use hank_provider::{CompletionRequest, ContentBlock, LlmProvider, StopReason, StreamEvent};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_stream::{Stream, StreamExt};
use tokio_util::sync::CancellationToken;
use tracing::warn;

/// 最大重试次数。
/// 口径取舍（P3-#16）：书中教学值为 Claude Code 的 10 次，本实现取 3 次——
/// agent loop 内每一步 LLM 调用已有外层 iteration 兜底，重试过多会拉长
/// 失败反馈延迟；瞬态错误通常 1-2 次内恢复，3 次足够覆盖。
const MAX_RETRIES: u32 = 3;
/// 基础退避时间（毫秒）。
/// 口径取舍（P3-#16）：书中为 500ms，本实现取 1000ms——配合 Retry-After
/// 优先策略（见下），自算退避只作兜底，稍保守的基数对限流端更友好。
const BASE_DELAY_MS: u64 = 1000;
/// 最大退避时间（毫秒）
const MAX_DELAY_MS: u64 = 30_000;

/// 贯穿一次 LLM 请求、重试与响应的关联信息。
#[derive(Debug, Clone)]
pub(crate) struct LlmTraceContext {
    pub(crate) call_id: String,
    pub(crate) run_id: Option<String>,
    pub(crate) turn_id: Option<String>,
    pub(crate) model: String,
    pub(crate) provider: String,
    pub(crate) phase: String,
}

pub(crate) async fn emit_llm_request(
    event_tx: &mpsc::Sender<AgentEvent>,
    trace: &LlmTraceContext,
    req: &CompletionRequest,
) {
    let _ = event_tx
        .send(AgentEvent::LlmRequest {
            call_id: trace.call_id.clone(),
            run_id: trace.run_id.clone(),
            turn_id: trace.turn_id.clone(),
            model: req.model.clone(),
            provider: trace.provider.clone(),
            system: req.system.clone(),
            messages: req.messages.clone(),
            tools: req.tools.iter().map(|tool| tool.name.clone()).collect(),
            tool_definitions: req.tools.clone(),
            max_tokens: req.max_tokens,
            message_count: req.messages.len(),
            phase: trace.phase.clone(),
        })
        .await;
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn emit_llm_response(
    event_tx: &mpsc::Sender<AgentEvent>,
    trace: &LlmTraceContext,
    content: Vec<ContentBlock>,
    stop_reason: StopReason,
    input_tokens: u32,
    output_tokens: u32,
    cache_read_tokens: u32,
    cache_write_tokens: u32,
    latency_ms: u64,
    cancelled: bool,
    timed_out: bool,
) {
    let _ = event_tx
        .send(AgentEvent::LlmResponse {
            call_id: trace.call_id.clone(),
            run_id: trace.run_id.clone(),
            turn_id: trace.turn_id.clone(),
            model: trace.model.clone(),
            provider: trace.provider.clone(),
            phase: trace.phase.clone(),
            content,
            stop_reason,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            latency_ms,
            cancelled,
            timed_out,
        })
        .await;
}

async fn emit_retry(
    event_tx: &mpsc::Sender<AgentEvent>,
    trace: &LlmTraceContext,
    stage: &str,
    failed_attempt: u32,
    delay: Duration,
    error: &anyhow::Error,
) {
    let _ = event_tx
        .send(AgentEvent::LlmRetry {
            call_id: trace.call_id.clone(),
            run_id: trace.run_id.clone(),
            turn_id: trace.turn_id.clone(),
            model: trace.model.clone(),
            provider: trace.provider.clone(),
            phase: trace.phase.clone(),
            stage: stage.to_string(),
            failed_attempt,
            next_attempt: failed_attempt + 1,
            delay_ms: delay.as_millis() as u64,
            error: error.to_string(),
        })
        .await;
}

async fn emit_failed(
    event_tx: &mpsc::Sender<AgentEvent>,
    trace: &LlmTraceContext,
    stage: &str,
    attempt: u32,
    retryable: bool,
    error: &anyhow::Error,
) {
    let _ = event_tx
        .send(AgentEvent::LlmFailed {
            call_id: trace.call_id.clone(),
            run_id: trace.run_id.clone(),
            turn_id: trace.turn_id.clone(),
            model: trace.model.clone(),
            provider: trace.provider.clone(),
            phase: trace.phase.clone(),
            stage: stage.to_string(),
            attempt,
            retryable,
            error: error.to_string(),
        })
        .await;
}

/// 判断错误是否可重试（瞬态错误）
fn is_retryable(error: &anyhow::Error) -> bool {
    let msg = error.to_string().to_lowercase();
    // HTTP 429 Too Many Requests
    if msg.contains("429") || msg.contains("rate limit") || msg.contains("too many requests") {
        return true;
    }
    // HTTP 5xx Server Errors — 精确匹配（防止 5000/50000 等误命中）
    if has_http_status(&msg, "500")
        || has_http_status(&msg, "502")
        || has_http_status(&msg, "503")
        || has_http_status(&msg, "504")
    {
        return true;
    }
    if msg.contains("internal server error")
        || msg.contains("bad gateway")
        || msg.contains("service unavailable")
        || msg.contains("gateway timeout")
    {
        return true;
    }
    // 网络错误
    if msg.contains("connection")
        || msg.contains("timeout")
        || msg.contains("timed out")
        || msg.contains("dns")
        || msg.contains("reset")
        || msg.contains("broken pipe")
    {
        return true;
    }
    if msg.contains("overloaded") {
        return true;
    }
    false
}

/// 检查 msg 中是否包含独立的 HTTP 状态码（不被其他数字包围）
fn has_http_status(msg: &str, code: &str) -> bool {
    let code_b = code.as_bytes();
    let msg_b = msg.as_bytes();
    let n = code_b.len();
    let mut i = 0;
    while i + n <= msg_b.len() {
        if &msg_b[i..i + n] == code_b {
            let before_ok = i == 0 || !msg_b[i - 1].is_ascii_digit();
            let after_ok = i + n >= msg_b.len() || !msg_b[i + n].is_ascii_digit();
            if before_ok && after_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// 计算退避延迟（指数退避 + 抖动）。
/// 口径取舍（P3-#16）：书中为 Equal Jitter（±25% 乘性），本实现用加性
/// 0-50% 抖动（delay ∈ [base, 1.5×base]）——实现更简单且同样能打散
/// 并发重试的同步效应；均值比 Equal Jitter 略高，与保守的 BASE_DELAY_MS 一致。
fn retry_delay(attempt: u32) -> std::time::Duration {
    let exponential = BASE_DELAY_MS * 2u64.pow(attempt);
    // 50% 随机抖动
    let jitter = (rand_jitter() * exponential as f64 * 0.5) as u64;
    let delay = (exponential + jitter).min(MAX_DELAY_MS);
    std::time::Duration::from_millis(delay)
}

/// 尽力从错误信息中解析 Retry-After 提示（秒）。
/// 优先采用服务端建议的等待时间，避免指数退避过于激进（loop.md:69）。
/// 上限 60s，防止异常大值阻塞循环。
fn retry_after_secs(error: &anyhow::Error) -> Option<u64> {
    let msg = error.to_string().to_lowercase();
    for marker in ["retry-after: ", "retry-after ", "retry after "] {
        if let Some(pos) = msg.find(marker) {
            let rest = &msg[pos + marker.len()..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(secs) = digits.parse::<u64>() {
                if secs > 0 {
                    return Some(secs.min(60));
                }
            }
        }
    }
    None
}

/// 简单的伪随机抖动 (0.0..1.0)，避免引入 rand crate
fn rand_jitter() -> f64 {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    (nanos % 1000) as f64 / 1000.0
}

/// 带重试的 LLM stream 调用。
/// 对瞬态错误（429、5xx、网络错误）自动重试，指数退避 + 抖动。
pub(crate) async fn stream_with_retry(
    provider: &Arc<dyn LlmProvider>,
    req: CompletionRequest,
    event_tx: &mpsc::Sender<AgentEvent>,
    trace: &LlmTraceContext,
) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
    let mut last_error = None;

    for attempt in 0..=MAX_RETRIES {
        match provider.stream(req.clone()).await {
            Ok(stream) => return Ok(stream),
            Err(e) => {
                if attempt < MAX_RETRIES && is_retryable(&e) {
                    // 优先采用服务端 Retry-After 提示，否则指数退避 + 抖动
                    let delay = retry_after_secs(&e)
                        .map(std::time::Duration::from_secs)
                        .unwrap_or_else(|| retry_delay(attempt));
                    warn!(
                        "LLM stream attempt {}/{} failed (retryable): {}. Retrying in {:?}",
                        attempt + 1,
                        MAX_RETRIES + 1,
                        e,
                        delay
                    );
                    emit_retry(event_tx, trace, "request", attempt + 1, delay, &e).await;
                    tokio::time::sleep(delay).await;
                    last_error = Some(e);
                    continue;
                }
                // 不可重试或已达最大重试次数
                if attempt == MAX_RETRIES {
                    warn!("LLM stream failed after {} retries: {}", MAX_RETRIES + 1, e);
                }
                emit_failed(
                    event_tx,
                    trace,
                    "request",
                    attempt + 1,
                    is_retryable(&e),
                    &e,
                )
                .await;
                return Err(e);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("LLM stream failed after retries")))
}

/// 一步 LLM 调用的流消费结果（本步累积状态）
pub(crate) struct StreamOutcome {
    pub assistant_content: Vec<ContentBlock>,
    pub stop_reason: StopReason,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_write_tokens: u32,
    /// 消费期间收到取消信号
    pub cancelled: bool,
    /// 流超时——非正常结束，调用方必须按失败/告警处理，
    /// 不能落入 EndTurn 正常收尾路径（【SA 03】）
    pub timed_out: bool,
}

/// 步骤级重试的流消费（【SA 03】）：把"发请求 + 消费流"包成一个可重试单元。
/// 消费中遇到可重试错误（连接 reset 等）时丢弃本步累积状态重来，最多 MAX_RETRIES 次；
/// 不可重试错误直接返回 Err。超时/取消只设置标志，由调用方决定如何收尾。
pub(crate) async fn consume_stream_with_retry(
    provider: &Arc<dyn LlmProvider>,
    req: CompletionRequest,
    event_tx: &mpsc::Sender<AgentEvent>,
    cancel: &CancellationToken,
    timeout: Duration,
    trace: &LlmTraceContext,
) -> Result<StreamOutcome> {
    let mut attempt = 0u32;
    let started_at = Instant::now();
    emit_llm_request(event_tx, trace, &req).await;

    'step: loop {
        let mut stream = stream_with_retry(provider, req.clone(), event_tx, trace).await?;

        let mut assistant_content: Vec<ContentBlock> = Vec::new();
        let mut current_text = String::new();
        let mut current_tool_id = String::new();
        let mut current_tool_name = String::new();
        let mut current_tool_input = String::new();
        let mut stop_reason = StopReason::EndTurn;
        let mut in_tool_block = false;
        let mut cancelled = false;
        let mut timed_out = false;
        let mut input_tokens = 0u32;
        let mut output_tokens = 0u32;
        let mut cache_read_tokens = 0u32;
        let mut cache_write_tokens = 0u32;

        loop {
            let event = tokio::select! {
                event = stream.next() => event,
                _ = cancel.cancelled() => {
                    cancelled = true;
                    None
                }
                _ = tokio::time::sleep(timeout) => {
                    warn!("LLM stream timeout after {timeout:?}");
                    timed_out = true;
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
                    input_tokens: i,
                    output_tokens: o,
                    cache_read_tokens: cache_read,
                    cache_write_tokens: cache_write,
                }) => {
                    input_tokens += i;
                    output_tokens += o;
                    cache_read_tokens += cache_read;
                    cache_write_tokens += cache_write;
                }
                Ok(StreamEvent::Error(msg)) => {
                    let _ = event_tx.send(AgentEvent::Error { message: msg }).await;
                }
                Err(e) => {
                    if attempt < MAX_RETRIES && is_retryable(&e) {
                        let delay = retry_after_secs(&e)
                            .map(Duration::from_secs)
                            .unwrap_or_else(|| retry_delay(attempt));
                        warn!(
                            "LLM stream consume attempt {}/{} failed (retryable): {}. \
                             丢弃本步累积状态，{:?} 后重试",
                            attempt + 1,
                            MAX_RETRIES + 1,
                            e,
                            delay
                        );
                        emit_retry(event_tx, trace, "stream", attempt + 1, delay, &e).await;
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                        continue 'step;
                    }
                    // 不可重试或已达最大重试次数
                    emit_failed(event_tx, trace, "stream", attempt + 1, is_retryable(&e), &e).await;
                    return Err(e);
                }
            }
        }

        // Flush remaining text
        if !current_text.is_empty() {
            assistant_content.push(ContentBlock::Text { text: current_text });
        }

        let outcome = StreamOutcome {
            assistant_content,
            stop_reason,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            cancelled,
            timed_out,
        };
        emit_llm_response(
            event_tx,
            trace,
            outcome.assistant_content.clone(),
            outcome.stop_reason,
            outcome.input_tokens,
            outcome.output_tokens,
            outcome.cache_read_tokens,
            outcome.cache_write_tokens,
            started_at.elapsed().as_millis() as u64,
            outcome.cancelled,
            outcome.timed_out,
        )
        .await;
        return Ok(outcome);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retryable_errors() {
        let err_429 = anyhow::anyhow!("HTTP 429 Too Many Requests");
        assert!(is_retryable(&err_429));

        let err_503 = anyhow::anyhow!("503 Service Unavailable");
        assert!(is_retryable(&err_503));

        let err_timeout = anyhow::anyhow!("connection timed out");
        assert!(is_retryable(&err_timeout));

        let err_400 = anyhow::anyhow!("400 Bad Request: invalid model");
        assert!(!is_retryable(&err_400));

        let err_auth = anyhow::anyhow!("401 Unauthorized");
        assert!(!is_retryable(&err_auth));
    }

    #[test]
    fn test_retry_delay_increases() {
        let d0 = retry_delay(0);
        let d1 = retry_delay(1);
        let d2 = retry_delay(2);
        // 指数增长（忽略抖动，基础值应递增）
        assert!(d1.as_millis() >= d0.as_millis());
        assert!(d2.as_millis() >= d1.as_millis());
        // 不超过上限
        let d10 = retry_delay(10);
        assert!(d10.as_millis() <= MAX_DELAY_MS as u128 + 1);
    }
}
