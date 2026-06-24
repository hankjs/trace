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
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
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
