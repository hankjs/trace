use super::summary::{estimate_tokens, microcompact, summarize_messages, summarize_with_llm};
use hank_provider::{ContentBlock, LlmProvider, Message, Role};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tracing::debug;

/// Default token threshold before compression triggers
const DEFAULT_TOKEN_THRESHOLD: usize = 80_000;
/// Number of recent messages to preserve during compression
const PRESERVE_RECENT: usize = 6;
/// Microcompact 保留的最近工具结果数（【SA 11】最近 3 个 ToolResult，按结果计数非消息数）
const MICROCOMPACT_PRESERVE_RESULTS: usize = 3;
/// Default total budget for context
const TOTAL_BUDGET_DEFAULT: usize = 200_000;

/// Budget status at different thresholds
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetStatus {
    /// Under 80% - normal operation
    Normal,
    /// 80-95% - warning, trigger compression
    Warning80,
    /// 95-100% - critical, force aggressive compression
    Critical95,
    /// Over 100% - overflow, must terminate
    Overflow100,
}

/// Compression strategy applied
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionStrategy {
    /// Microcompact: truncate old ToolResult blocks
    Microcompact,
    /// LLM summarization: use LLM to summarize middle messages
    Summarize,
    /// Truncate: remove oldest messages
    Truncate,
}

/// 纯文本 user 消息（非 tool_results）——压缩保留段的合法起点（【SA 11】）
fn is_plain_user_message(msg: &Message) -> bool {
    msg.role == Role::User
        && !msg.content.is_empty()
        && msg
            .content
            .iter()
            .all(|b| matches!(b, ContentBlock::Text { .. }))
}

/// 将 recent 窗口起点向新消息方向对齐到纯文本 user 消息边界。
/// 保留段不能以 assistant 或 tool_results user 消息开头，否则 API 报错（【SA 11】）。
/// 找不到合法边界时返回 None，调用方应在切分后跑配对修复 pass。
fn align_to_user_boundary(messages: &[Message], start: usize) -> Option<usize> {
    (start..messages.len()).find(|&i| is_plain_user_message(&messages[i]))
}

/// 配对修复 pass（【AF 17】）：删孤儿 tool_result（对应 tool_use 已被压缩掉），
/// 给孤儿 tool_use 补占位 tool_result；被清空的消息整条移除。
fn repair_tool_pairing(messages: &mut Vec<Message>) {
    use std::collections::HashSet;

    // 1) 收集保留段内所有 tool_use id
    let tool_use_ids: HashSet<String> = messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();

    // 2) 删除孤儿 tool_result；清空的消息整条移除
    for msg in messages.iter_mut() {
        msg.content.retain(|b| match b {
            ContentBlock::ToolResult { tool_use_id, .. } => tool_use_ids.contains(tool_use_id),
            _ => true,
        });
    }
    messages.retain(|m| !m.content.is_empty());

    // 3) 给孤儿 tool_use 补占位 tool_result（并入紧随的 user 消息，或新建一条）
    let mut i = 0;
    while i < messages.len() {
        if messages[i].role == Role::Assistant {
            let ids: Vec<String> = messages[i]
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolUse { id, .. } => Some(id.clone()),
                    _ => None,
                })
                .collect();
            if !ids.is_empty() {
                let next_is_user = i + 1 < messages.len() && messages[i + 1].role == Role::User;
                let answered: HashSet<String> = if next_is_user {
                    messages[i + 1]
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::ToolResult { tool_use_id, .. } => {
                                Some(tool_use_id.clone())
                            }
                            _ => None,
                        })
                        .collect()
                } else {
                    HashSet::new()
                };
                let placeholders: Vec<ContentBlock> = ids
                    .into_iter()
                    .filter(|id| !answered.contains(id))
                    .map(|id| ContentBlock::ToolResult {
                        tool_use_id: id,
                        content: "[tool result lost during context compression]".to_string(),
                        is_error: true,
                    })
                    .collect();
                if !placeholders.is_empty() {
                    if next_is_user {
                        let tail = std::mem::take(&mut messages[i + 1].content);
                        messages[i + 1].content =
                            placeholders.into_iter().chain(tail).collect();
                    } else {
                        messages.insert(
                            i + 1,
                            Message {
                                role: Role::User,
                                content: placeholders,
                            },
                        );
                    }
                }
            }
        }
        i += 1;
    }
}

/// Manages context window by estimating tokens and compressing when needed.
pub struct ContextManager {
    token_threshold: usize,
    total_budget: usize,
    provider: Option<Arc<dyn LlmProvider>>,
    model: Option<String>,
    /// 由 provider 报告的实际 token 用量（累积输入 token）
    actual_input_tokens: Option<usize>,
    /// 自上次 actual 校准后新 push 消息的估算 token（粗估增量，【SA 12】）。
    /// 用 AtomicUsize 以便在 &self 的调用方（如 WorkerAgent）中累加，且保持 Send+Sync。
    pending_estimated: AtomicUsize,
}

impl ContextManager {
    pub fn new() -> Self {
        Self {
            token_threshold: DEFAULT_TOKEN_THRESHOLD,
            total_budget: TOTAL_BUDGET_DEFAULT,
            provider: None,
            model: None,
            actual_input_tokens: None,
            pending_estimated: AtomicUsize::new(0),
        }
    }

    pub fn with_threshold(threshold: usize) -> Self {
        Self {
            token_threshold: threshold,
            total_budget: TOTAL_BUDGET_DEFAULT,
            provider: None,
            model: None,
            actual_input_tokens: None,
            pending_estimated: AtomicUsize::new(0),
        }
    }

    /// Create a ContextManager with LLM-based compression support.
    pub fn with_provider(
        threshold: usize,
        provider: Arc<dyn LlmProvider>,
        model: String,
    ) -> Self {
        Self {
            token_threshold: threshold,
            total_budget: TOTAL_BUDGET_DEFAULT,
            provider: Some(provider),
            model: Some(model),
            actual_input_tokens: None,
            pending_estimated: AtomicUsize::new(0),
        }
    }

    /// Create a ContextManager with custom budget settings
    pub fn with_budget(
        threshold: usize,
        total_budget: usize,
        provider: Arc<dyn LlmProvider>,
        model: String,
    ) -> Self {
        Self {
            token_threshold: threshold,
            total_budget,
            provider: Some(provider),
            model: Some(model),
            actual_input_tokens: None,
            pending_estimated: AtomicUsize::new(0),
        }
    }

    /// 更新 provider 报告的实际 input token 用量。
    /// 当有实际值时，check_budget 会优先使用它（精确基准 + 粗估增量）。
    pub fn update_actual_tokens(&mut self, input_tokens: usize) {
        self.actual_input_tokens = Some(input_tokens);
        // 精确基准已校准，清零粗估增量（【SA 12】）
        self.pending_estimated.store(0, Ordering::Relaxed);
    }

    /// 压缩后重置实际 token 计数（估算值已变化）
    pub fn reset_actual_tokens(&mut self) {
        self.actual_input_tokens = None;
        self.pending_estimated.store(0, Ordering::Relaxed);
    }

    /// 累加新 push 消息的估算 token（actual 校准点之后的粗估增量）。
    /// 每次 push 新消息后调用，避免巨大的工具结果要等到下一次 LLM 调用才被察觉。
    pub fn add_pending(&self, tokens: usize) {
        self.pending_estimated.fetch_add(tokens, Ordering::Relaxed);
    }

    /// 总预算（用于事件上报，避免硬编码 200_000）
    pub fn total_budget(&self) -> usize {
        self.total_budget
    }

    /// Check budget status based on current token usage.
    /// 有 actual 时用"精确基准 + 粗估增量"（【SA 12】），否则使用全量估算值。
    pub fn check_budget(&self, messages: &[Message]) -> BudgetStatus {
        let used = self
            .actual_input_tokens
            .map(|actual| actual + self.pending_estimated.load(Ordering::Relaxed))
            .unwrap_or_else(|| estimate_tokens(messages));
        let percent = ((used as f64 / self.total_budget as f64) * 100.0) as u32;

        if used >= self.total_budget {
            BudgetStatus::Overflow100
        } else if percent >= 95 {
            BudgetStatus::Critical95
        } else if percent >= 80 {
            BudgetStatus::Warning80
        } else {
            BudgetStatus::Normal
        }
    }

    /// Check if messages exceed the token threshold.
    pub fn needs_compression(&self, messages: &[Message]) -> bool {
        estimate_tokens(messages) > self.token_threshold
    }

    /// Compress messages: keep first message + recent N messages,
    /// replace middle with a summary message.
    pub fn compress(&self, messages: &mut Vec<Message>) {
        if messages.len() <= PRESERVE_RECENT + 1 {
            return; // Not enough messages to compress
        }

        let estimated = estimate_tokens(messages);
        debug!(
            "Context compression triggered: ~{estimated} tokens, {} messages",
            messages.len()
        );

        // Keep first message (original request) and last N messages.
        // 切分点向新消息方向对齐到纯文本 user 消息边界（【SA 11】），
        // 无法对齐时在重建后跑配对修复 pass（【AF 17】）。
        let start = messages.len() - PRESERVE_RECENT;
        let (cut, needs_repair) = match align_to_user_boundary(messages, start) {
            Some(aligned) => (aligned, false),
            None => (start, true),
        };
        let first = messages[0].clone();
        let middle = &messages[1..cut];
        let summary_text = summarize_messages(middle);

        let summary_msg = Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: format!(
                    "[Context Summary - previous {} messages compressed]\n{}",
                    middle.len(),
                    summary_text
                ),
            }],
        };

        let recent: Vec<Message> = messages[cut..].to_vec();

        messages.clear();
        messages.push(first);
        messages.push(summary_msg);
        messages.extend(recent);
        if needs_repair {
            repair_tool_pairing(messages);
        }

        debug!(
            "After compression: {} messages, ~{} tokens",
            messages.len(),
            estimate_tokens(messages)
        );
    }

    /// Three-layer compression pipeline:
    /// Layer 1: microcompact (memory op, always succeeds)
    /// Layer 2: LLM summarization (can fail, optional)
    /// Layer 3: truncate_oldest (fallback, always succeeds)
    /// Returns the strategy applied, if any.
    pub async fn compress_async(&self, messages: &mut Vec<Message>) -> Option<CompressionStrategy> {
        if messages.len() <= PRESERVE_RECENT + 1 {
            return None;
        }

        let before_tokens = estimate_tokens(messages);
        debug!(
            "Context compression triggered: ~{} tokens, {} messages",
            before_tokens,
            messages.len()
        );

        // Layer 1: Microcompact
        let saved = microcompact(messages, MICROCOMPACT_PRESERVE_RESULTS);
        debug!("Microcompact saved ~{} tokens", saved);

        let after_layer1 = estimate_tokens(messages);
        if after_layer1 <= self.token_threshold {
            debug!(
                "Microcompact sufficient: {} -> {} tokens",
                before_tokens, after_layer1
            );
            return Some(CompressionStrategy::Microcompact);
        }

        // Layer 2: LLM Summarization (optional)
        if let (Some(provider), Some(model)) = (&self.provider, &self.model) {
            if messages.len() > PRESERVE_RECENT + 1 {
                // 切分点对齐到纯文本 user 消息边界（【SA 11】），同 compress()
                let start = messages.len() - PRESERVE_RECENT;
                let (cut, needs_repair) = match align_to_user_boundary(messages, start) {
                    Some(aligned) => (aligned, false),
                    None => (start, true),
                };
                let first = messages[0].clone();
                let middle = &messages[1..cut];
                let summary_text = summarize_with_llm(middle, provider.as_ref(), model).await;

                let summary_msg = Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text {
                        text: format!(
                            "[Context Summary - previous {} messages compressed]\n{}",
                            middle.len(),
                            summary_text
                        ),
                    }],
                };

                let recent: Vec<Message> = messages[cut..].to_vec();

                messages.clear();
                messages.push(first);
                messages.push(summary_msg);
                messages.extend(recent);
                if needs_repair {
                    repair_tool_pairing(messages);
                }

                let after_layer2 = estimate_tokens(messages);
                if after_layer2 <= self.token_threshold {
                    debug!(
                        "LLM summarization sufficient: {} -> {} tokens",
                        after_layer1, after_layer2
                    );
                    return Some(CompressionStrategy::Summarize);
                }
            }
        }

        // Layer 3: Truncate oldest (fallback)
        self.truncate_oldest(messages);
        let after_layer3 = estimate_tokens(messages);
        debug!(
            "Truncate fallback: {} -> {} tokens",
            after_layer1, after_layer3
        );
        Some(CompressionStrategy::Truncate)
    }

    /// Remove oldest messages to reduce token count (fallback strategy).
    /// 切分点同样对齐到纯文本 user 消息边界，无法对齐时跑配对修复（【SA 11】【AF 17】）。
    fn truncate_oldest(&self, messages: &mut Vec<Message>) {
        if messages.len() <= 2 {
            return;
        }
        // Keep first and last N messages
        let keep = (PRESERVE_RECENT + 1).min(messages.len());
        if messages.len() > keep {
            let start = messages.len() - keep;
            match align_to_user_boundary(messages, start) {
                Some(aligned) => {
                    let tail: Vec<Message> = messages[aligned..].to_vec();
                    messages.clear();
                    messages.extend(tail);
                }
                None => {
                    let tail: Vec<Message> = messages[start..].to_vec();
                    messages.clear();
                    messages.extend(tail);
                    repair_tool_pairing(messages);
                }
            }
        }
    }
}

impl Default for ContextManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_text(text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    fn assistant_tool_use(id: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({"path": format!("{id}.txt")}),
            }],
        }
    }

    fn user_tool_result(id: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: format!("content of {id}"),
                is_error: false,
            }],
        }
    }

    /// 断言无孤儿 tool_use/tool_result：每个 result 的 tool_use 在前面，
    /// 每个 tool_use 的 result 在后面
    fn assert_pairing_intact(messages: &[Message]) {
        use std::collections::HashSet;
        let mut open: HashSet<String> = HashSet::new();
        let mut answered: HashSet<String> = HashSet::new();
        for msg in messages {
            for block in &msg.content {
                match block {
                    ContentBlock::ToolUse { id, .. } => {
                        open.insert(id.clone());
                    }
                    ContentBlock::ToolResult { tool_use_id, .. } => {
                        assert!(
                            open.contains(tool_use_id),
                            "孤儿 tool_result: {tool_use_id}"
                        );
                        answered.insert(tool_use_id.clone());
                    }
                    _ => {}
                }
            }
        }
        for id in &open {
            assert!(answered.contains(id), "孤儿 tool_use: {id}");
        }
    }

    /// #6 验收：压缩切分点对齐到 user 纯文本消息边界，无孤儿 tool_use/tool_result
    #[test]
    fn test_compress_aligns_to_user_boundary() {
        // 0: user 纯文本；1-8: 四对 assistant(tool_use)/user(tool_result)；
        // 9: user 纯文本；10-13: 两对 tool 消息。
        // 朴素切分 start = 14 - 6 = 8 落在 tool_result（孤儿）上；
        // 对齐后应从 index 9 的 user 纯文本开始保留。
        let mut messages = vec![user_text("original request")];
        for i in 1..=4 {
            messages.push(assistant_tool_use(&format!("t{i}")));
            messages.push(user_tool_result(&format!("t{i}")));
        }
        messages.push(user_text("mid-conversation user input"));
        for i in 5..=6 {
            messages.push(assistant_tool_use(&format!("t{i}")));
            messages.push(user_tool_result(&format!("t{i}")));
        }
        assert_eq!(messages.len(), 14);

        let cm = ContextManager::new();
        cm.compress(&mut messages);

        // first + summary + 对齐后的 recent（user 纯文本开头）
        assert!(messages.len() >= 3);
        // 第一条非 summary 消息必须是 user 纯文本
        let first_kept = &messages[2];
        assert!(
            is_plain_user_message(first_kept),
            "first kept message should be plain user text, got {:?}",
            first_kept.role
        );
        assert_pairing_intact(&messages);
    }

    /// #6：无法对齐（保留窗口内无 user 纯文本）时，配对修复 pass 删掉孤儿 tool_result
    #[test]
    fn test_compress_repair_pass_removes_orphan_tool_result() {
        let mut messages = vec![user_text("original request")];
        for i in 1..=6 {
            messages.push(assistant_tool_use(&format!("t{i}")));
            messages.push(user_tool_result(&format!("t{i}")));
        }
        // len = 13，朴素切分 start = 13 - 6 = 7，落在 assistant(t4)；
        // 窗口 7..13 内无 user 纯文本 → 走修复 pass（起点 assistant 合法，
        // 但 t4 之后若窗口从 tool_result 开始则会产生孤儿）。
        let cm = ContextManager::new();
        cm.compress(&mut messages);
        assert_pairing_intact(&messages);
    }

    /// #6：修复 pass 给孤儿 tool_use 补占位 result
    #[test]
    fn test_repair_tool_pairing_fills_orphan_tool_use() {
        let mut messages = vec![
            user_text("req"),
            assistant_tool_use("t1"),
            assistant_tool_use("t2"), // 两条 assistant 相邻：t1 的 result 丢失
            user_tool_result("t2"),
        ];
        repair_tool_pairing(&mut messages);
        assert_pairing_intact(&messages);
    }

    /// #7 验收：actual=100K（预算 200K），push 估算 150K 的工具结果后 Overflow100
    #[test]
    fn test_check_budget_uses_actual_plus_pending() {
        let mut cm = ContextManager::new(); // 默认预算 200K
        cm.update_actual_tokens(100_000);
        cm.add_pending(150_000);
        // messages 为空：若只用 actual（旧行为）会返回 Normal
        assert_eq!(cm.check_budget(&[]), BudgetStatus::Overflow100);
    }

    /// #7：update_actual_tokens 校准时清零 pending 增量
    #[test]
    fn test_update_actual_tokens_clears_pending() {
        let mut cm = ContextManager::new();
        cm.update_actual_tokens(100_000);
        cm.add_pending(150_000);
        // 新一轮 LLM 响应校准基准：pending 清零，回到 100K → Normal
        cm.update_actual_tokens(100_000);
        assert_eq!(cm.check_budget(&[]), BudgetStatus::Normal);
    }
}
