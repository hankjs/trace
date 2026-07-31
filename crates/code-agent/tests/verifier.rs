//! Verifier 集成测试：流事件消费与 verdict 解析。
//! 使用脚本化 MockProvider 驱动 VerifierAgent，无需真实 LLM。

use async_trait::async_trait;
use code_agent::agent::verifier::VerifierAgent;
use code_agent::Verdict;
use code_tools::Tool;
use futures::Stream;
use hank_provider::{CompletionRequest, LlmProvider, StopReason, StreamEvent};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// 一次 LLM 响应的脚本：产出的 stream 事件序列。
type Script = Vec<StreamEvent>;

/// 按调用次数依次返回预设脚本的 mock provider。
struct MockProvider {
    scripts: Mutex<std::collections::VecDeque<Script>>,
}

impl MockProvider {
    fn new(scripts: Vec<Script>) -> Self {
        Self {
            scripts: Mutex::new(scripts.into_iter().collect()),
        }
    }
}

#[async_trait]
impl LlmProvider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }

    async fn stream(
        &self,
        _req: CompletionRequest,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>> {
        let script = self.scripts.lock().unwrap().pop_front().unwrap_or_else(|| {
            vec![StreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
            }]
        });
        let events: Vec<anyhow::Result<StreamEvent>> = script.into_iter().map(Ok).collect();
        Ok(Box::pin(futures::stream::iter(events)))
    }
}

/// Anthropic provider 在 message_start 阶段第一个事件就发 Usage，
/// Verifier 必须忽略它继续消费后续文本，而不是当作流结束（否则验证恒为空转）。
#[tokio::test]
async fn test_verifier_ignores_leading_usage_event() {
    let script = vec![
        StreamEvent::Usage {
            input_tokens: 100,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        },
        StreamEvent::TextDelta(
            r#"{"verdict": "needs_revision", "issues": ["missing tests"]}"#.to_string(),
        ),
        StreamEvent::MessageEnd {
            stop_reason: StopReason::EndTurn,
        },
    ];
    let provider = Arc::new(MockProvider::new(vec![script]));
    let tools: Vec<Arc<dyn Tool>> = vec![];
    let verifier = VerifierAgent::new(provider, tools, "mock-model".to_string());

    let (tx, _rx) = mpsc::channel(64);
    let result = verifier
        .verify(
            "test-run",
            "original request",
            "task summary",
            tx,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    assert!(
        matches!(result.verdict, Verdict::NeedsRevision),
        "verdict={:?}",
        result.verdict
    );
    assert_eq!(result.issues, vec!["missing tests".to_string()]);
}
