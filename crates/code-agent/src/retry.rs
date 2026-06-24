use anyhow::Result;
use hank_provider::{CompletionRequest, LlmProvider, StreamEvent};
use std::pin::Pin;
use std::sync::Arc;
use tokio_stream::Stream;
use tracing::warn;

/// 最大重试次数
const MAX_RETRIES: u32 = 3;
/// 基础退避时间（毫秒）
const BASE_DELAY_MS: u64 = 1000;
/// 最大退避时间（毫秒）
const MAX_DELAY_MS: u64 = 30_000;

/// 判断错误是否可重试（瞬态错误）
fn is_retryable(error: &anyhow::Error) -> bool {
    let msg = error.to_string().to_lowercase();
    // HTTP 429 Too Many Requests
    if msg.contains("429") || msg.contains("rate limit") || msg.contains("too many requests") {
        return true;
    }
    // HTTP 5xx Server Errors — 精确匹配（防止 5000/50000 等误命中）
    if has_http_status(&msg, "500") || has_http_status(&msg, "502")
        || has_http_status(&msg, "503") || has_http_status(&msg, "504")
    {
        return true;
    }
    if msg.contains("internal server error") || msg.contains("bad gateway")
        || msg.contains("service unavailable") || msg.contains("gateway timeout") {
        return true;
    }
    // 网络错误
    if msg.contains("connection") || msg.contains("timeout") || msg.contains("timed out")
        || msg.contains("dns") || msg.contains("reset") || msg.contains("broken pipe") {
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

/// 计算退避延迟（指数退避 + 抖动）
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
pub async fn stream_with_retry(
    provider: &Arc<dyn LlmProvider>,
    req: CompletionRequest,
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
                    tokio::time::sleep(delay).await;
                    last_error = Some(e);
                    continue;
                }
                // 不可重试或已达最大重试次数
                if attempt == MAX_RETRIES {
                    warn!(
                        "LLM stream failed after {} retries: {}",
                        MAX_RETRIES + 1,
                        e
                    );
                }
                return Err(e);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("LLM stream failed after retries")))
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
