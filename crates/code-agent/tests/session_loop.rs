//! Simple Mode 闭环集成测试：run/turn 生命周期、权限拒绝、文件变更事件。
//! 使用脚本化 MockProvider 驱动 AgentSession，无需真实 LLM。

use async_trait::async_trait;
use code_agent::{AgentEvent, AgentSession, FileChangeKind, ThinkStrategy};
use code_tools::{
    write_file::WriteFileTool, PermissionConfig, PermissionMode, Tool, ToolOutput, ToolRisk,
};
use futures::Stream;
use hank_provider::{CompletionRequest, LlmProvider, StopReason, StreamEvent};
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// 一次 LLM 响应的脚本：产出的 stream 事件序列。
type Script = Vec<StreamEvent>;

/// 按调用次数依次返回预设脚本的 mock provider。
/// 约定：空脚本表示挂起的 stream（用于测试超时路径）。
struct MockProvider {
    scripts: Mutex<std::collections::VecDeque<Vec<anyhow::Result<StreamEvent>>>>,
    /// stream() 被调用的次数（用于断言外层循环是否真正终止）
    calls: Arc<AtomicUsize>,
}

impl MockProvider {
    fn new(scripts: Vec<Script>) -> Self {
        Self {
            scripts: Mutex::new(
                scripts
                    .into_iter()
                    .map(|s| s.into_iter().map(Ok).collect())
                    .collect(),
            ),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// 支持脚本中混入 Err 事件（用于测试流消费中的重试）
    fn new_raw(scripts: Vec<Vec<anyhow::Result<StreamEvent>>>) -> Self {
        Self {
            scripts: Mutex::new(scripts.into_iter().collect()),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
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
        self.calls.fetch_add(1, Ordering::SeqCst);
        let script = self.scripts.lock().unwrap().pop_front().unwrap_or_else(|| {
            vec![Ok(StreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
            })]
        });
        // 空脚本：挂起的 stream（既不产出事件也不结束），用于测试超时
        if script.is_empty() {
            return Ok(Box::pin(futures::stream::pending()));
        }
        Ok(Box::pin(futures::stream::iter(script)))
    }
}

struct DangerousNoopTool {
    executed: Arc<AtomicBool>,
}

#[async_trait]
impl Tool for DangerousNoopTool {
    fn name(&self) -> &str {
        "dangerous_noop"
    }

    fn description(&self) -> &str {
        "Test-only dangerous tool."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    fn risk_level(&self) -> ToolRisk {
        ToolRisk::Dangerous
    }

    async fn execute(&self, _input: serde_json::Value) -> anyhow::Result<ToolOutput> {
        self.executed.store(true, Ordering::SeqCst);
        Ok(ToolOutput {
            content: "executed".to_string(),
            is_error: false,
        })
    }
}

struct StreamingEchoTool;

#[async_trait]
impl Tool for StreamingEchoTool {
    fn name(&self) -> &str {
        "streaming_echo"
    }

    fn description(&self) -> &str {
        "Test-only streaming tool."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    fn supports_streaming(&self) -> bool {
        true
    }

    async fn execute(&self, _input: serde_json::Value) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput {
            content: "fallback".to_string(),
            is_error: false,
        })
    }

    async fn execute_streaming(
        &self,
        _input: serde_json::Value,
        stream_tx: mpsc::Sender<String>,
    ) -> anyhow::Result<ToolOutput> {
        let _ = stream_tx.send("chunk-a".to_string()).await;
        let _ = stream_tx.send("chunk-b".to_string()).await;
        Ok(ToolOutput {
            content: "stream complete".to_string(),
            is_error: false,
        })
    }
}

/// 收集所有事件直到 channel 关闭。
async fn collect_events(mut rx: mpsc::Receiver<AgentEvent>) -> Vec<AgentEvent> {
    let mut out = Vec::new();
    while let Some(ev) = rx.recv().await {
        out.push(ev);
    }
    out
}

fn tool_use_script(id: &str, name: &str, input_json: &str) -> Script {
    vec![
        StreamEvent::ToolUseStart {
            id: id.to_string(),
            name: name.to_string(),
        },
        StreamEvent::ToolUseInputDelta(input_json.to_string()),
        StreamEvent::ToolUseEnd,
        StreamEvent::MessageEnd {
            stop_reason: StopReason::ToolUse,
        },
        StreamEvent::Usage {
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        },
    ]
}

fn text_end_script(text: &str) -> Script {
    vec![
        StreamEvent::TextDelta(text.to_string()),
        StreamEvent::MessageEnd {
            stop_reason: StopReason::EndTurn,
        },
        StreamEvent::Usage {
            input_tokens: 120,
            output_tokens: 10,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        },
    ]
}

#[tokio::test]
async fn test_run_turn_lifecycle_text_only() {
    let provider = Arc::new(MockProvider::new(vec![text_end_script("done")]));
    let tools: Vec<Arc<dyn Tool>> = vec![];
    let mut session =
        AgentSession::new(provider, tools, "mock-model".to_string(), "sys".to_string());

    let (tx, rx) = mpsc::channel(64);
    let cancel = CancellationToken::new();
    session
        .run(
            vec![hank_provider::ContentBlock::Text {
                text: "hi".to_string(),
            }],
            tx,
            cancel,
        )
        .await
        .unwrap();

    let events = collect_events(rx).await;

    // 必须包含 run.started / turn.started / turn.completed / run.completed / TurnComplete
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::RunStarted { .. })),
        "missing run.started"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnStarted { .. })),
        "missing turn.started"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnCompleted { .. })),
        "missing turn.completed"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::RunCompleted { .. })),
        "missing run.completed"
    );

    // RunStarted 应在最前，TurnComplete 应在最后
    assert!(matches!(
        events.first().unwrap(),
        AgentEvent::RunStarted { .. }
    ));
    assert!(matches!(events.last().unwrap(), AgentEvent::TurnComplete));

    // RunCompleted 出现在 TurnComplete 之前
    let run_completed_idx = events
        .iter()
        .position(|e| matches!(e, AgentEvent::RunCompleted { .. }))
        .unwrap();
    let turn_complete_idx = events
        .iter()
        .position(|e| matches!(e, AgentEvent::TurnComplete))
        .unwrap();
    assert!(run_completed_idx < turn_complete_idx);
}

#[tokio::test]
async fn test_file_changed_event_on_write() {
    let dir = tempdir_path();
    let provider = Arc::new(MockProvider::new(vec![
        tool_use_script("t1", "write_file", r#"{"path":"hello.txt","content":"hi"}"#),
        text_end_script("wrote file"),
    ]));
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(WriteFileTool::new(Some(dir.clone())))];
    let mut session =
        AgentSession::new(provider, tools, "mock-model".to_string(), "sys".to_string())
            .with_permission(code_tools::PermissionMode::WorkspaceWrite, dir.clone());

    let (tx, rx) = mpsc::channel(64);
    session
        .run(
            vec![hank_provider::ContentBlock::Text {
                text: "make a file".to_string(),
            }],
            tx,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let events = collect_events(rx).await;

    // 应发出 file.changed (Add)
    let fc = events.iter().find_map(|e| match e {
        AgentEvent::FileChanged { changes, .. } => Some(changes.clone()),
        _ => None,
    });
    let changes = fc.expect("missing file.changed event");
    assert_eq!(changes.len(), 1);
    assert!(changes[0].path.contains("hello.txt"));

    // run.completed.summary 应提及变更文件
    let summary = events.iter().find_map(|e| match e {
        AgentEvent::RunCompleted {
            summary,
            file_changes,
            ..
        } => Some((summary.clone(), file_changes.clone())),
        _ => None,
    });
    let (summary, file_changes) = summary.expect("missing run.completed");
    assert!(summary.contains("hello.txt"), "summary={summary}");
    assert_eq!(file_changes.len(), 1);

    // 实际文件应被写入
    assert!(std::path::Path::new(&format!("{dir}/hello.txt")).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_file_changed_event_marks_existing_write_as_update() {
    let dir = tempdir_path();
    std::fs::write(format!("{dir}/existing.txt"), "old").unwrap();
    let provider = Arc::new(MockProvider::new(vec![
        tool_use_script(
            "t1",
            "write_file",
            r#"{"path":"existing.txt","content":"new"}"#,
        ),
        text_end_script("updated file"),
    ]));
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(WriteFileTool::new(Some(dir.clone())))];
    let mut session =
        AgentSession::new(provider, tools, "mock-model".to_string(), "sys".to_string())
            .with_permission(code_tools::PermissionMode::WorkspaceWrite, dir.clone());

    let (tx, rx) = mpsc::channel(64);
    session
        .run(
            vec![hank_provider::ContentBlock::Text {
                text: "update a file".to_string(),
            }],
            tx,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let events = collect_events(rx).await;
    let changes = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::FileChanged { changes, .. } => Some(changes.clone()),
            _ => None,
        })
        .expect("missing file.changed event");

    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].kind, FileChangeKind::Update);
    assert_eq!(
        std::fs::read_to_string(format!("{dir}/existing.txt")).unwrap(),
        "new"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_permission_denied_outside_sandbox() {
    let dir = tempdir_path();
    let provider = Arc::new(MockProvider::new(vec![
        tool_use_script(
            "t1",
            "write_file",
            r#"{"path":"/etc/evil.txt","content":"x"}"#,
        ),
        text_end_script("could not write"),
    ]));
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(WriteFileTool::new(Some(dir.clone())))];
    let mut session =
        AgentSession::new(provider, tools, "mock-model".to_string(), "sys".to_string())
            .with_permission(code_tools::PermissionMode::WorkspaceWrite, dir.clone());

    let (tx, rx) = mpsc::channel(64);
    session
        .run(
            vec![hank_provider::ContentBlock::Text {
                text: "write outside".to_string(),
            }],
            tx,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let events = collect_events(rx).await;

    // 必须发出 permission.denied
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::PermissionDenied { .. })),
        "missing permission.denied"
    );

    // run.completed.permission_denials 非空
    let denials = events.iter().find_map(|e| match e {
        AgentEvent::RunCompleted {
            permission_denials, ..
        } => Some(permission_denials.clone()),
        _ => None,
    });
    assert!(!denials.expect("missing run.completed").is_empty());

    // 不应写出 /etc/evil.txt
    assert!(!std::path::Path::new("/etc/evil.txt").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_read_only_mode_denies_write() {
    let dir = tempdir_path();
    let provider = Arc::new(MockProvider::new(vec![
        tool_use_script("t1", "write_file", r#"{"path":"a.txt","content":"x"}"#),
        text_end_script("denied"),
    ]));
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(WriteFileTool::new(Some(dir.clone())))];
    let mut session =
        AgentSession::new(provider, tools, "mock-model".to_string(), "sys".to_string())
            .with_permission(code_tools::PermissionMode::ReadOnly, dir.clone());

    let (tx, rx) = mpsc::channel(64);
    session
        .run(
            vec![hank_provider::ContentBlock::Text {
                text: "write".to_string(),
            }],
            tx,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let events = collect_events(rx).await;
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::PermissionDenied { .. })));
    assert!(!std::path::Path::new(&format!("{dir}/a.txt")).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_escalated_dangerous_tool_requests_and_denies_without_execution() {
    let dir = tempdir_path();
    let executed = Arc::new(AtomicBool::new(false));
    let provider = Arc::new(MockProvider::new(vec![
        tool_use_script("t1", "dangerous_noop", r#"{}"#),
        text_end_script("not executed"),
    ]));
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(DangerousNoopTool {
        executed: executed.clone(),
    })];
    let mut config = PermissionConfig::default();
    config.mode = PermissionMode::Escalated;
    config.sandbox_paths = vec![dir.clone()];
    let mut session =
        AgentSession::new(provider, tools, "mock-model".to_string(), "sys".to_string())
            .with_permission_config(config, dir.clone());

    let (tx, rx) = mpsc::channel(64);
    session
        .run(
            vec![hank_provider::ContentBlock::Text {
                text: "run dangerous tool".to_string(),
            }],
            tx,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let events = collect_events(rx).await;
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::PermissionRequested { .. })));
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::PermissionDenied { .. })));
    assert!(!events
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolStart { name, .. } if name == "dangerous_noop")));
    assert!(!executed.load(Ordering::SeqCst));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_streaming_tool_output_delta_is_forwarded() {
    let provider = Arc::new(MockProvider::new(vec![
        tool_use_script("t1", "streaming_echo", r#"{}"#),
        text_end_script("streamed"),
    ]));
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(StreamingEchoTool)];
    let mut session =
        AgentSession::new(provider, tools, "mock-model".to_string(), "sys".to_string());

    let (tx, rx) = mpsc::channel(64);
    session
        .run(
            vec![hank_provider::ContentBlock::Text {
                text: "stream".to_string(),
            }],
            tx,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let events = collect_events(rx).await;
    let chunks: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolOutputDelta { id, chunk } if id == "t1" => Some(chunk.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(chunks, vec!["chunk-a".to_string(), "chunk-b".to_string()]);
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ToolResult {
            id,
            content,
            is_error: false,
        } if id == "t1" && content == "stream complete"
    )));
}

#[tokio::test]
async fn test_orchestrated_file_changed_event_on_write() {
    let dir = tempdir_path();
    let provider = Arc::new(MockProvider::new(vec![
        tool_use_script("t1", "write_file", r#"{"path":"orch.txt","content":"hi"}"#),
        text_end_script("done"),
    ]));
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(WriteFileTool::new(Some(dir.clone())))];
    let mut session = AgentSession::orchestrated(
        provider,
        tools,
        "mock-model".to_string(),
        "sys".to_string(),
        ThinkStrategy::Never,
    )
    .with_permission(code_tools::PermissionMode::WorkspaceWrite, dir.clone());

    let (tx, rx) = mpsc::channel(64);
    session
        .run(
            vec![hank_provider::ContentBlock::Text {
                text: "make a file".to_string(),
            }],
            tx,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let events = collect_events(rx).await;

    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::FileChanged { changes, .. } if changes.iter().any(|c| c.path.contains("orch.txt"))
    )));
    let file_changes = events.iter().find_map(|e| match e {
        AgentEvent::RunCompleted { file_changes, .. } => Some(file_changes.clone()),
        _ => None,
    });
    assert_eq!(file_changes.expect("missing run.completed").len(), 1);
    assert!(std::path::Path::new(&format!("{dir}/orch.txt")).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_orchestrated_read_only_mode_denies_write() {
    let dir = tempdir_path();
    let provider = Arc::new(MockProvider::new(vec![
        tool_use_script(
            "t1",
            "write_file",
            r#"{"path":"orch-denied.txt","content":"x"}"#,
        ),
        text_end_script("denied"),
    ]));
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(WriteFileTool::new(Some(dir.clone())))];
    let mut session = AgentSession::orchestrated(
        provider,
        tools,
        "mock-model".to_string(),
        "sys".to_string(),
        ThinkStrategy::Never,
    )
    .with_permission(code_tools::PermissionMode::ReadOnly, dir.clone());

    let (tx, rx) = mpsc::channel(64);
    session
        .run(
            vec![hank_provider::ContentBlock::Text {
                text: "write".to_string(),
            }],
            tx,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let events = collect_events(rx).await;
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::PermissionDenied { .. })));
    let denials = events.iter().find_map(|e| match e {
        AgentEvent::RunCompleted {
            permission_denials, ..
        } => Some(permission_denials.clone()),
        _ => None,
    });
    assert!(!denials.expect("missing run.completed").is_empty());
    assert!(!std::path::Path::new(&format!("{dir}/orch-denied.txt")).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// 生成一个唯一的临时目录路径并创建。
fn tempdir_path() -> String {
    let base = std::env::temp_dir();
    let unique = format!("code-agent-test-{}", uuid::Uuid::new_v4());
    let path = base.join(unique);
    std::fs::create_dir_all(&path).unwrap();
    path.to_string_lossy().to_string()
}

/// 一次响应中携带多个 tool_use 块的脚本。
fn multi_tool_use_script(ids: &[&str], name: &str, input_json: &str) -> Script {
    let mut events = Vec::new();
    for id in ids {
        events.push(StreamEvent::ToolUseStart {
            id: id.to_string(),
            name: name.to_string(),
        });
        events.push(StreamEvent::ToolUseInputDelta(input_json.to_string()));
        events.push(StreamEvent::ToolUseEnd);
    }
    events.push(StreamEvent::MessageEnd {
        stop_reason: StopReason::ToolUse,
    });
    events
}

/// 循环检测 terminate：一条 assistant 消息含多个 tool_use 时，
/// 每个 tool_use 都必须有配对 tool_result，且外层循环真正终止（不再发起 LLM 调用）。
#[tokio::test]
async fn test_loop_terminate_pairs_all_tool_results_and_stops() {
    // 无进展 streak ≥10 触发全局熔断：第 1 轮 5 次相同调用 + 第 2 轮 6 次相同调用，
    // 第 10 次（t10）触发 terminate，此时 t11 是同一条 assistant 消息中尚未执行的 tool_use。
    let provider = Arc::new(MockProvider::new(vec![
        multi_tool_use_script(&["t1", "t2", "t3", "t4", "t5"], "streaming_echo", r#"{}"#),
        multi_tool_use_script(
            &["t6", "t7", "t8", "t9", "t10", "t11"],
            "streaming_echo",
            r#"{}"#,
        ),
        text_end_script("should not reach here"),
    ]));
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(StreamingEchoTool)];
    let mut session =
        AgentSession::new(provider.clone(), tools, "mock-model".to_string(), "sys".to_string());

    let (tx, rx) = mpsc::channel(64);
    session
        .run(
            vec![hank_provider::ContentBlock::Text {
                text: "loop".to_string(),
            }],
            tx,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let events = collect_events(rx).await;

    // 应发出 loop.detected
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::LoopDetected { .. })),
        "missing loop.detected"
    );

    // terminate 后外层循环真正终止：只消费了 2 个脚本，第 3 个不应被请求
    assert_eq!(
        provider.call_count(),
        2,
        "terminate 后不应再发起下一轮 LLM 调用"
    );

    // messages 中每个 tool_use 都有对应的 tool_result
    let mut tool_use_ids: Vec<String> = Vec::new();
    let mut tool_result_ids: Vec<String> = Vec::new();
    for msg in session.messages() {
        for block in &msg.content {
            match block {
                hank_provider::ContentBlock::ToolUse { id, .. } => tool_use_ids.push(id.clone()),
                hank_provider::ContentBlock::ToolResult { tool_use_id, .. } => {
                    tool_result_ids.push(tool_use_id.clone())
                }
                _ => {}
            }
        }
    }
    assert_eq!(tool_use_ids.len(), 11, "tool_use_ids={tool_use_ids:?}");
    for id in &tool_use_ids {
        assert!(
            tool_result_ids.contains(id),
            "tool_use {id} 缺少配对的 tool_result"
        );
    }

    // 未执行的 t11 收到的是 abort 错误结果
    let t11_result = session.messages().iter().find_map(|msg| {
        msg.content.iter().find_map(|block| match block {
            hank_provider::ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } if tool_use_id == "t11" => Some((content.clone(), *is_error)),
            _ => None,
        })
    });
    let (content, is_error) = t11_result.expect("missing tool_result for t11");
    assert!(is_error);
    assert!(content.contains("Loop detected"), "content={content}");
}

/// MaxTokens 截断发生在完整 tool_use 块之后的脚本。
fn tool_use_max_tokens_script(id: &str, name: &str, input_json: &str) -> Script {
    vec![
        StreamEvent::ToolUseStart {
            id: id.to_string(),
            name: name.to_string(),
        },
        StreamEvent::ToolUseInputDelta(input_json.to_string()),
        StreamEvent::ToolUseEnd,
        StreamEvent::MessageEnd {
            stop_reason: StopReason::MaxTokens,
        },
    ]
}

/// #5 验收：MaxTokens 截断时 assistant_content 中含完整 tool_use 块，
/// 照常执行工具并保持配对（方案 A），不注入续写提示。
#[tokio::test]
async fn test_max_tokens_executes_completed_tool_use() {
    let provider = Arc::new(MockProvider::new(vec![
        tool_use_max_tokens_script("t1", "streaming_echo", r#"{}"#),
        text_end_script("done"),
    ]));
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(StreamingEchoTool)];
    let mut session =
        AgentSession::new(provider.clone(), tools, "mock-model".to_string(), "sys".to_string());

    let (tx, rx) = mpsc::channel(64);
    session
        .run(
            vec![hank_provider::ContentBlock::Text {
                text: "hi".to_string(),
            }],
            tx,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let events = collect_events(rx).await;

    // run 成功完成
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::RunCompleted {
            status: code_agent::RunStatus::Success,
            ..
        }
    )));
    assert_eq!(provider.call_count(), 2);

    // 每个 tool_use 都有配对 tool_result
    let mut tool_use_ids: Vec<String> = Vec::new();
    let mut tool_result_ids: Vec<String> = Vec::new();
    for msg in session.messages() {
        for block in &msg.content {
            match block {
                hank_provider::ContentBlock::ToolUse { id, .. } => tool_use_ids.push(id.clone()),
                hank_provider::ContentBlock::ToolResult { tool_use_id, .. } => {
                    tool_result_ids.push(tool_use_id.clone())
                }
                _ => {}
            }
        }
    }
    assert_eq!(tool_use_ids, vec!["t1".to_string()]);
    assert!(tool_result_ids.contains(&"t1".to_string()));

    // 未注入续写提示
    let has_continuation_prompt = session.messages().iter().any(|msg| {
        msg.content.iter().any(|b| matches!(
            b,
            hank_provider::ContentBlock::Text { text } if text.contains("cut off")
        ))
    });
    assert!(!has_continuation_prompt, "不应注入续写提示");
}

/// #8 验收：流消费中途发生可重试错误时，丢弃本步累积状态重试，
/// 重试后成功且消息里无半截内容。
#[tokio::test]
async fn test_stream_error_step_retry_discards_partial_state() {
    let provider = Arc::new(MockProvider::new_raw(vec![
        // 第 1 次：半截文本后连接 reset（可重试）
        vec![
            Ok(StreamEvent::TextDelta("partial ".to_string())),
            Err(anyhow::anyhow!("connection reset by peer")),
        ],
        // 第 2 次（重试）：正常完成
        text_end_script("done").into_iter().map(Ok).collect(),
    ]));
    let tools: Vec<Arc<dyn Tool>> = vec![];
    let mut session =
        AgentSession::new(provider.clone(), tools, "mock-model".to_string(), "sys".to_string());

    let (tx, rx) = mpsc::channel(64);
    session
        .run(
            vec![hank_provider::ContentBlock::Text {
                text: "hi".to_string(),
            }],
            tx,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let events = collect_events(rx).await;

    // 重试后 run 成功
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::RunCompleted {
            status: code_agent::RunStatus::Success,
            ..
        }
    )));
    assert_eq!(provider.call_count(), 2, "可重试错误应触发步骤级重试");

    // assistant 消息无半截内容（本步累积状态已丢弃）
    let assistant_text: String = session
        .messages()
        .iter()
        .filter(|m| matches!(m.role, hank_provider::Role::Assistant))
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            hank_provider::ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(!assistant_text.contains("partial"), "assistant_text={assistant_text}");
    assert!(assistant_text.contains("done"));
}

/// #8 验收：流超时（挂起的 stream）以失败/超时事件收尾，
/// 不能静默落入 RunCompleted(Success)。
#[tokio::test]
async fn test_stream_timeout_fails_run() {
    // 空脚本 = 挂起的 stream（既不产出事件也不结束）
    let provider = Arc::new(MockProvider::new(vec![vec![]]));
    let tools: Vec<Arc<dyn Tool>> = vec![];
    let mut session =
        AgentSession::new(provider, tools, "mock-model".to_string(), "sys".to_string())
            .with_stream_timeout(std::time::Duration::from_millis(200));

    let (tx, rx) = mpsc::channel(64);
    let result = session
        .run(
            vec![hank_provider::ContentBlock::Text {
                text: "hi".to_string(),
            }],
            tx,
            CancellationToken::new(),
        )
        .await;
    assert!(result.is_err(), "超时应以失败收尾");

    let events = collect_events(rx).await;

    // 带原因的错误事件
    assert!(
        events.iter().any(|e| matches!(
            e,
            AgentEvent::Error { message } if message.contains("timed out")
        )),
        "missing timeout error event"
    );
    // RunFailed 收尾，而非 RunCompleted(Success)
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::RunFailed { .. })),
        "missing run.failed"
    );
    assert!(!events.iter().any(|e| matches!(
        e,
        AgentEvent::RunCompleted {
            status: code_agent::RunStatus::Success,
            ..
        }
    )));
}

/// 测试用工具：执行时取消 CancellationToken（模拟工具间取消路径）。
struct CancellingTool {
    cancel: CancellationToken,
}

#[async_trait]
impl Tool for CancellingTool {
    fn name(&self) -> &str {
        "cancelling_tool"
    }

    fn description(&self) -> &str {
        "Test-only tool that cancels the run."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(&self, _input: serde_json::Value) -> anyhow::Result<ToolOutput> {
        self.cancel.cancel();
        Ok(ToolOutput {
            content: "cancelling".to_string(),
            is_error: false,
        })
    }
}

/// #13 验收：工具间取消时，已执行的工具结果保留、未执行的补占位 result。
#[tokio::test]
async fn test_cancel_between_tools_preserves_pairing() {
    let cancel = CancellationToken::new();
    let provider = Arc::new(MockProvider::new(vec![multi_tool_use_script(
        &["t1", "t2"],
        "cancelling_tool",
        r#"{}"#,
    )]));
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(CancellingTool {
        cancel: cancel.clone(),
    })];
    let mut session =
        AgentSession::new(provider, tools, "mock-model".to_string(), "sys".to_string());

    let (tx, rx) = mpsc::channel(64);
    session
        .run(
            vec![hank_provider::ContentBlock::Text {
                text: "hi".to_string(),
            }],
            tx,
            cancel,
        )
        .await
        .unwrap();

    let events = collect_events(rx).await;

    // 取消收尾
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::RunCancelled { .. })),
        "missing run.cancelled"
    );

    // t1 已执行：保留真实结果；t2 未执行：补占位错误 result
    let find_result = |id: &str| {
        session.messages().iter().find_map(|msg| {
            msg.content.iter().find_map(|b| match b {
                hank_provider::ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } if tool_use_id == id => Some((content.clone(), *is_error)),
                _ => None,
            })
        })
    };
    let (c1, e1) = find_result("t1").expect("missing tool_result for t1");
    assert_eq!(c1, "cancelling");
    assert!(!e1);
    let (c2, e2) = find_result("t2").expect("missing placeholder tool_result for t2");
    assert!(e2);
    assert!(c2.contains("cancelled before execution"), "content={c2}");
}

/// #13 验收：ask_user 暂停时，已执行的工具结果保留、其余补占位，
/// ask_user 的 tool_use 不预填结果（留给 server 端用用户答案回填）。
#[tokio::test]
async fn test_ask_user_pause_preserves_pairing() {
    let script = vec![
        StreamEvent::ToolUseStart {
            id: "a1".to_string(),
            name: "streaming_echo".to_string(),
        },
        StreamEvent::ToolUseInputDelta(r#"{}"#.to_string()),
        StreamEvent::ToolUseEnd,
        StreamEvent::ToolUseStart {
            id: "ask1".to_string(),
            name: "ask_user".to_string(),
        },
        StreamEvent::ToolUseInputDelta(r#"{"question":"继续吗？"}"#.to_string()),
        StreamEvent::ToolUseEnd,
        StreamEvent::ToolUseStart {
            id: "a2".to_string(),
            name: "streaming_echo".to_string(),
        },
        StreamEvent::ToolUseInputDelta(r#"{}"#.to_string()),
        StreamEvent::ToolUseEnd,
        StreamEvent::MessageEnd {
            stop_reason: StopReason::ToolUse,
        },
    ];
    let provider = Arc::new(MockProvider::new(vec![script]));
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(StreamingEchoTool)];
    let mut session =
        AgentSession::new(provider, tools, "mock-model".to_string(), "sys".to_string());

    let (tx, rx) = mpsc::channel(64);
    session
        .run(
            vec![hank_provider::ContentBlock::Text {
                text: "hi".to_string(),
            }],
            tx,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let events = collect_events(rx).await;

    // ask_user 事件发出；paused 不发 run 终态
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::AskUser { tool_use_id, .. } if tool_use_id == "ask1")));
    assert!(!events
        .iter()
        .any(|e| matches!(e, AgentEvent::RunCompleted { .. } | AgentEvent::RunFailed { .. })));

    let find_result = |id: &str| {
        session.messages().iter().find_map(|msg| {
            msg.content.iter().find_map(|b| match b {
                hank_provider::ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } if tool_use_id == id => Some((content.clone(), *is_error)),
                _ => None,
            })
        })
    };
    // a1 已执行：保留真实结果
    assert!(find_result("a1").is_some(), "a1 的结果应保留");
    // ask_user：不预填结果（由 server 端用用户答案回填）
    assert!(find_result("ask1").is_none(), "ask_user 不应预填结果");
    // a2 未执行：补占位错误 result
    let (c2, e2) = find_result("a2").expect("missing placeholder tool_result for a2");
    assert!(e2);
    assert!(c2.contains("ask_user interrupted"), "content={c2}");
}

/// #14 验收：首次调用 deferred 工具时不执行盲猜参数，
/// 返回 "Tool schema now loaded" 错误结果让模型带完整 schema 重试。
#[tokio::test]
async fn test_deferred_tool_first_call_returns_retry_hint() {
    let dir = tempdir_path();
    let provider = Arc::new(MockProvider::new(vec![
        // 第 1 次：空 schema 盲猜调用
        tool_use_script("t1", "write_file", r#"{"path":"deferred.txt","content":"x"}"#),
        // 第 2 次：拿到完整 schema 后重试
        tool_use_script("t2", "write_file", r#"{"path":"deferred.txt","content":"x"}"#),
        text_end_script("done"),
    ]));
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(WriteFileTool::new(Some(dir.clone())))];
    let mut session =
        AgentSession::new(provider, tools, "mock-model".to_string(), "sys".to_string())
            .with_permission(code_tools::PermissionMode::WorkspaceWrite, dir.clone())
            .with_deferred_tools(["write_file"]);

    let (tx, rx) = mpsc::channel(64);
    session
        .run(
            vec![hank_provider::ContentBlock::Text {
                text: "write a file".to_string(),
            }],
            tx,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let _events = collect_events(rx).await;

    let find_result = |id: &str| {
        session.messages().iter().find_map(|msg| {
            msg.content.iter().find_map(|b| match b {
                hank_provider::ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } if tool_use_id == id => Some((content.clone(), *is_error)),
                _ => None,
            })
        })
    };
    // 第 1 次盲猜：不执行，返回重试提示
    let (c1, e1) = find_result("t1").expect("missing tool_result for t1");
    assert!(e1);
    assert!(c1.contains("Tool schema now loaded"), "content={c1}");
    // 第 2 次重试：正常执行，文件写入
    let (_c2, e2) = find_result("t2").expect("missing tool_result for t2");
    assert!(!e2);
    assert!(std::path::Path::new(&format!("{dir}/deferred.txt")).exists());
    let _ = std::fs::remove_dir_all(&dir);
}
