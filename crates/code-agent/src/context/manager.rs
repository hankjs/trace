use super::summary::{estimate_tokens, microcompact, summarize_messages, summarize_with_llm};
use hank_provider::{ContentBlock, LlmProvider, Message, Role};
use std::sync::Arc;
use tracing::debug;

/// Default token threshold before compression triggers
const DEFAULT_TOKEN_THRESHOLD: usize = 80_000;
/// Number of recent messages to preserve during compression
const PRESERVE_RECENT: usize = 6;
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

/// Manages context window by estimating tokens and compressing when needed.
pub struct ContextManager {
    token_threshold: usize,
    total_budget: usize,
    provider: Option<Arc<dyn LlmProvider>>,
    model: Option<String>,
    /// 由 provider 报告的实际 token 用量（累积输入 token）
    actual_input_tokens: Option<usize>,
}

impl ContextManager {
    pub fn new() -> Self {
        Self {
            token_threshold: DEFAULT_TOKEN_THRESHOLD,
            total_budget: TOTAL_BUDGET_DEFAULT,
            provider: None,
            model: None,
            actual_input_tokens: None,
        }
    }

    pub fn with_threshold(threshold: usize) -> Self {
        Self {
            token_threshold: threshold,
            total_budget: TOTAL_BUDGET_DEFAULT,
            provider: None,
            model: None,
            actual_input_tokens: None,
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
        }
    }

    /// 更新 provider 报告的实际 input token 用量。
    /// 当有实际值时，check_budget 会优先使用它。
    pub fn update_actual_tokens(&mut self, input_tokens: usize) {
        self.actual_input_tokens = Some(input_tokens);
    }

    /// 压缩后重置实际 token 计数（估算值已变化）
    pub fn reset_actual_tokens(&mut self) {
        self.actual_input_tokens = None;
    }

    /// 总预算（用于事件上报，避免硬编码 200_000）
    pub fn total_budget(&self) -> usize {
        self.total_budget
    }

    /// Check budget status based on current token usage.
    /// 优先使用 provider 报告的实际 token 数，否则使用估算值。
    pub fn check_budget(&self, messages: &[Message]) -> BudgetStatus {
        let used = self.actual_input_tokens.unwrap_or_else(|| estimate_tokens(messages));
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

        // Keep first message (original request) and last N messages
        let first = messages[0].clone();
        let middle = &messages[1..messages.len() - PRESERVE_RECENT];
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

        let recent: Vec<Message> =
            messages[messages.len() - PRESERVE_RECENT..].to_vec();

        messages.clear();
        messages.push(first);
        messages.push(summary_msg);
        messages.extend(recent);

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
        let saved = microcompact(messages, PRESERVE_RECENT);
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
                let first = messages[0].clone();
                let middle = &messages[1..messages.len() - PRESERVE_RECENT];
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

                let recent: Vec<Message> =
                    messages[messages.len() - PRESERVE_RECENT..].to_vec();

                messages.clear();
                messages.push(first);
                messages.push(summary_msg);
                messages.extend(recent);

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

    /// Remove oldest messages to reduce token count (fallback strategy)
    fn truncate_oldest(&self, messages: &mut Vec<Message>) {
        if messages.len() <= 2 {
            return;
        }
        // Keep first and last N messages
        let keep = (PRESERVE_RECENT + 1).min(messages.len());
        if messages.len() > keep {
            let tail: Vec<Message> = messages[messages.len() - keep..].to_vec();
            messages.clear();
            messages.extend(tail);
        }
    }
}

impl Default for ContextManager {
    fn default() -> Self {
        Self::new()
    }
}
