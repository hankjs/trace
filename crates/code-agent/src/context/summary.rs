use hank_provider::{CompletionRequest, ContentBlock, LlmProvider, Message, StreamEvent};
use tokio_stream::StreamExt;
use tracing::{debug, warn};

/// 工具结果截断的默认上限（字符数）
const TOOL_RESULT_MAX_CHARS: usize = 40_000;

/// UTF-8 安全截断：按字符边界截断到最多 `max_chars` 个字符
fn truncate_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// 截断工具结果：保留 60% head + 40% tail，中间插入截断提示。
/// 防止单次工具调用撑爆 context window。
/// 全程以「字符数」为单位，避免字节/字符口径混用导致 CJK 内容提前截断。
pub fn truncate_tool_result(content: &str, max_chars: usize) -> String {
    let total_chars = content.chars().count();
    if total_chars <= max_chars {
        return content.to_string();
    }
    let head_chars = max_chars * 60 / 100;
    let tail_chars = max_chars * 40 / 100;
    let omitted = total_chars - head_chars - tail_chars;

    let head: String = content.chars().take(head_chars).collect();
    let tail: String = content
        .chars()
        .skip(total_chars - tail_chars)
        .collect::<String>();
    format!(
        "{}\n\n...[truncated {} of {} chars]...\n\n{}",
        head, omitted, total_chars, tail
    )
}

/// 使用默认上限截断工具结果
pub fn truncate_tool_result_default(content: &str) -> String {
    truncate_tool_result(content, TOOL_RESULT_MAX_CHARS)
}

/// Token 估算：根据字符类型选择不同的分割系数。
/// - ASCII/Latin 字符：~4 chars/token
/// - CJK 字符（中日韩）：~1.5 chars/token
/// - Image：估算 1000 tokens（占位）
pub fn estimate_tokens(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|m| {
            m.content
                .iter()
                .map(|block| match block {
                    hank_provider::ContentBlock::Text { text } => estimate_text_tokens(text),
                    hank_provider::ContentBlock::Image { .. } => 1000,
                    hank_provider::ContentBlock::ToolUse { input, .. } => {
                        let s = input.to_string();
                        estimate_text_tokens(&s)
                    }
                    hank_provider::ContentBlock::ToolResult { content, .. } => {
                        estimate_text_tokens(content)
                    }
                })
                .sum::<usize>()
        })
        .sum::<usize>()
}

/// 估算单段文本的 token 数，区分 CJK 和 ASCII
fn estimate_text_tokens(text: &str) -> usize {
    let mut cjk_chars = 0usize;
    let mut other_bytes = 0usize;
    for ch in text.chars() {
        if is_cjk(ch) {
            cjk_chars += 1;
        } else {
            other_bytes += ch.len_utf8();
        }
    }
    // CJK: ~1.5 chars per token, ASCII: ~4 bytes per token
    let cjk_tokens = (cjk_chars as f64 / 1.5).ceil() as usize;
    let ascii_tokens = other_bytes / 4;
    cjk_tokens + ascii_tokens
}

/// 判断字符是否为 CJK 统一表意字符
fn is_cjk(ch: char) -> bool {
    matches!(ch,
        '\u{4E00}'..='\u{9FFF}' |   // CJK Unified Ideographs
        '\u{3400}'..='\u{4DBF}' |   // CJK Extension A
        '\u{F900}'..='\u{FAFF}' |   // CJK Compatibility Ideographs
        '\u{3000}'..='\u{303F}' |   // CJK Symbols and Punctuation
        '\u{3040}'..='\u{309F}' |   // Hiragana
        '\u{30A0}'..='\u{30FF}' |   // Katakana
        '\u{AC00}'..='\u{D7AF}'     // Hangul Syllables
    )
}

/// Generate a summary of messages for context compression.
/// This is a simple extractive summary — keeps key information.
pub fn summarize_messages(messages: &[Message]) -> String {
    let mut summary_parts = Vec::new();

    for msg in messages {
        let role = match msg.role {
            hank_provider::Role::User => "User",
            hank_provider::Role::Assistant => "Assistant",
        };

        for block in &msg.content {
            match block {
                hank_provider::ContentBlock::Text { text } => {
                    // Take first 200 chars of each text block
                    let truncated = if text.chars().count() > 200 {
                        format!("{}...", truncate_chars(text, 200))
                    } else {
                        text.clone()
                    };
                    summary_parts.push(format!("[{role}]: {truncated}"));
                }
                hank_provider::ContentBlock::Image { .. } => {
                    summary_parts.push(format!("[{role} sent an image]"));
                }
                hank_provider::ContentBlock::ToolUse { name, .. } => {
                    summary_parts.push(format!("[{role} used tool: {name}]"));
                }
                hank_provider::ContentBlock::ToolResult { content, is_error, .. } => {
                    let status = if *is_error { "error" } else { "ok" };
                    let truncated = if content.chars().count() > 100 {
                        format!("{}...", truncate_chars(content, 100))
                    } else {
                        content.clone()
                    };
                    summary_parts.push(format!("[Tool result ({status})]: {truncated}"));
                }
            }
        }
    }

    summary_parts.join("\n")
}

/// Microcompact messages: remove most content from old ToolResult blocks
/// while preserving structure. Keeps first 80 chars + original length info.
/// Returns estimated tokens saved.
pub fn microcompact(messages: &mut Vec<Message>, preserve_recent: usize) -> usize {
    if messages.len() <= preserve_recent + 1 {
        return 0;
    }

    let before_tokens = estimate_tokens(messages);
    let cutoff = messages.len().saturating_sub(preserve_recent);

    // Compact ToolResult blocks in all messages before the preserve window
    for msg in messages.iter_mut().take(cutoff) {
        for block in &mut msg.content {
            if let ContentBlock::ToolResult { content, tool_use_id: _, is_error: _ } = block {
                if content.chars().count() > 80 {
                    let original_len = content.len();
                    let first_80 = truncate_chars(content, 80).to_string();
                    *content = format!("{}...[truncated from {} chars]", first_80, original_len);
                }
            }
        }
    }

    let after_tokens = estimate_tokens(messages);
    (before_tokens.saturating_sub(after_tokens)).min(before_tokens)
}

/// Summarize messages using an LLM for higher-quality context compression.
/// Falls back to extractive `summarize_messages()` on failure.
pub async fn summarize_with_llm(
    messages: &[Message],
    provider: &dyn LlmProvider,
    model: &str,
) -> String {
    // Build extractive summary as input material (avoids sending raw messages which could be huge)
    let extractive = summarize_messages(messages);

    let prompt = format!(
        "You are a context compression assistant. Below is an extractive summary of a conversation \
         between a user and an AI assistant. Produce a concise but complete summary that preserves:\n\
         - The original user request/goal\n\
         - Key decisions made\n\
         - Files modified or created\n\
         - Errors encountered and approaches that failed (do NOT omit failures — \
           the agent must avoid repeating dead-ends)\n\
         - Current progress and state\n\
         - What remains to be done\n\n\
         Keep the summary under 1000 words. Be factual and specific.\n\n\
         --- EXTRACTIVE SUMMARY ---\n{extractive}\n--- END ---\n\n\
         Produce your compressed summary now:"
    );

    let req = CompletionRequest {
        model: model.to_string(),
        system: None,
        messages: vec![Message {
            role: hank_provider::Role::User,
            content: vec![hank_provider::ContentBlock::Text { text: prompt }],
        }],
        tools: vec![],
        max_tokens: 2048,
    };

    match provider.stream(req).await {
        Ok(mut stream) => {
            let mut result = String::new();
            while let Some(event) = stream.next().await {
                match event {
                    Ok(StreamEvent::TextDelta(text)) => {
                        result.push_str(&text);
                    }
                    Ok(StreamEvent::MessageEnd { .. }) => break,
                    Err(e) => {
                        warn!("LLM summarization stream error: {e}, falling back to extractive");
                        return extractive;
                    }
                    _ => {}
                }
            }
            if result.is_empty() {
                warn!("LLM summarization returned empty, falling back to extractive");
                return extractive;
            }
            debug!("LLM summarization produced {} chars", result.len());
            result
        }
        Err(e) => {
            warn!("LLM summarization request failed: {e}, falling back to extractive");
            extractive
        }
    }
}
