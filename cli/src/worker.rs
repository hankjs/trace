//! poll 主循环 + 工具 dispatch + 并发执行。
//! 对齐 client/src/composables/useRemoteExec.ts：正常返回立即下一轮（退避重置 1s），
//! 错误指数退避 1s→30s，401 时 relogin 后继续；一批请求 tokio::spawn 并发执行。

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use tokio::sync::mpsc;

use crate::agent::{AgentRunInput, AgentRunner};
use crate::api::{ApiClient, PollOutcome, ToolCallRequest};
use crate::notify::{self, NotifyTx};
use crate::terminal::TermManager;

/// read_file_base64 上限（微信媒体回传，与 src-tauri 一致）
const MAX_READ_FILE_BASE64_BYTES: u64 = 20 * 1024 * 1024;

const BACKOFF_INIT: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// 工具执行结果（回传 server 的 content/is_error）
struct ToolOutput {
    content: String,
    is_error: bool,
}

impl ToolOutput {
    fn ok(content: String) -> Self {
        Self {
            content,
            is_error: false,
        }
    }
    fn err(content: String) -> Self {
        Self {
            content,
            is_error: true,
        }
    }
}

/// 执行一条远程下发的工具调用
async fn execute_tool(
    term: &Arc<TermManager>,
    notify_tx: &NotifyTx,
    req: &ToolCallRequest,
) -> ToolOutput {
    let input = &req.input;
    match req.tool.as_str() {
        "terminal_create" => {
            // 远程开终端（微信托管会话用），默认值与 TS 版一致
            let cols = input.get("cols").and_then(|v| v.as_u64()).unwrap_or(120) as u16;
            let rows = input.get("rows").and_then(|v| v.as_u64()).unwrap_or(30) as u16;
            let cwd = input
                .get("cwd")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            match term.term_create(cols, rows, cwd, notify_tx.clone()) {
                Ok(info) => ToolOutput::ok(serde_json::to_string(&info).unwrap_or_default()),
                Err(e) => ToolOutput::err(format!("Remote exec error: {e}")),
            }
        }
        "terminal_read" => {
            let id = input.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if input.get("raw").and_then(|v| v.as_bool()).unwrap_or(false) {
                // raw 模式保留 ANSI；app 侧优先取 xterm 屏幕快照，CLI 直接走 PTY 原始流
                let max_bytes = input
                    .get("maxBytes")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(65536) as usize;
                match term.term_read(id, Some(max_bytes), Some(true)) {
                    Ok(text) => ToolOutput::ok(text),
                    Err(e) => ToolOutput::err(format!("Remote exec error: {e}")),
                }
            } else {
                let lines = input.get("lines").and_then(|v| v.as_u64()).unwrap_or(200) as usize;
                // 未指定 lines 时按字节兜底，避免返回过大；指定 lines 时读全量再按行截尾
                let max_bytes = if input.get("lines").is_some() {
                    None
                } else {
                    Some(40000)
                };
                match term.term_read(id, max_bytes, None) {
                    Ok(text) => {
                        let tail: Vec<&str> = text.split('\n').collect();
                        let start = tail.len().saturating_sub(lines);
                        ToolOutput::ok(tail[start..].join("\n"))
                    }
                    Err(e) => ToolOutput::err(format!("Remote exec error: {e}")),
                }
            }
        }
        "terminal_write" => {
            let id = input.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let data = input.get("data").and_then(|v| v.as_str()).unwrap_or("");
            match term.term_write(id, data) {
                Ok(()) => ToolOutput::ok("ok".into()),
                Err(e) => ToolOutput::err(format!("Remote exec error: {e}")),
            }
        }
        "terminal_close" => {
            let id = input.get("id").and_then(|v| v.as_str()).unwrap_or("");
            match term.term_close(id) {
                Ok(()) => ToolOutput::ok("ok".into()),
                Err(e) => ToolOutput::err(format!("Remote exec error: {e}")),
            }
        }
        "terminal_set_enabled" => {
            let id = input.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let enabled = input.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
            match term.term_set_enabled(id, enabled) {
                Ok(info) => ToolOutput::ok(serde_json::to_string(&info).unwrap_or_default()),
                Err(e) => ToolOutput::err(format!("Remote exec error: {e}")),
            }
        }
        "terminal_list" => {
            let list = term.term_list();
            ToolOutput::ok(serde_json::to_string(&list).unwrap_or_default())
        }
        "read_file_base64" => {
            // 微信渠道媒体回传：读取本地文件并以 base64 返回（二进制安全）
            let path = input.get("path").and_then(|v| v.as_str()).unwrap_or("");
            read_file_base64(path)
        }
        _ => ToolOutput::err(format!("Unknown tool: {}", req.tool)),
    }
}

async fn execute_request(
    api: Arc<ApiClient>,
    agent: Arc<AgentRunner>,
    term: &Arc<TermManager>,
    notify_tx: &NotifyTx,
    req: &ToolCallRequest,
) -> ToolOutput {
    match req.tool.as_str() {
        "agent_run" => {
            let input = match serde_json::from_value::<AgentRunInput>(req.input.clone()) {
                Ok(input) => input,
                Err(error) => return ToolOutput::err(format!("Invalid agent_run input: {error}")),
            };
            let outcome = agent.run(api, &req.request_id, input).await;
            ToolOutput {
                content: outcome.content,
                is_error: outcome.is_error,
            }
        }
        "agent_cancel" => {
            let request_id = req
                .input
                .get("request_id")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            if request_id.is_empty() {
                ToolOutput::err("agent_cancel 缺少 request_id".into())
            } else {
                agent.cancel(request_id).await;
                ToolOutput::ok("ok".into())
            }
        }
        _ => execute_tool(term, notify_tx, req).await,
    }
}

/// 读文件（≤20MB）base64 编码返回，超限/读失败返回 is_error（对齐 src-tauri tool_read_file_base64）
fn read_file_base64(path: &str) -> ToolOutput {
    let result = (|| -> Result<String, String> {
        let meta = std::fs::metadata(path).map_err(|e| format!("Error reading file: {e}"))?;
        if !meta.is_file() {
            return Err(format!("Not a regular file: {path}"));
        }
        if meta.len() > MAX_READ_FILE_BASE64_BYTES {
            return Err(format!(
                "File too large: {} bytes (limit {})",
                meta.len(),
                MAX_READ_FILE_BASE64_BYTES
            ));
        }
        let data = std::fs::read(path).map_err(|e| format!("Error reading file: {e}"))?;
        Ok(base64::engine::general_purpose::STANDARD.encode(&data))
    })();
    match result {
        Ok(content) => ToolOutput::ok(content),
        Err(e) => ToolOutput::err(e),
    }
}

/// 长轮询主循环：错误指数退避 1s→30s，401 重新登录
pub async fn run(
    api: Arc<ApiClient>,
    term: Arc<TermManager>,
    agent: Arc<AgentRunner>,
    agent_backends: Vec<String>,
    client_id: String,
) {
    let (notify_tx, notify_rx) = mpsc::unbounded_channel();
    // 通知上报任务：消费各终端 reader 线程捕获的 OSC/BEL 事件
    tokio::spawn(notify::run(api.clone(), client_id.clone(), notify_rx));

    let mut backoff = BACKOFF_INIT;
    loop {
        match api.poll(&client_id, &agent_backends).await {
            PollOutcome::Ok(requests) => {
                // 正常返回（含空 requests）立即进入下一轮
                backoff = BACKOFF_INIT;
                for req in requests {
                    let api = api.clone();
                    let term = term.clone();
                    let agent = agent.clone();
                    let notify_tx = notify_tx.clone();
                    tokio::spawn(async move {
                        let out =
                            execute_request(api.clone(), agent, &term, &notify_tx, &req).await;
                        if let Err(e) = api
                            .post_result(&req.request_id, &out.content, out.is_error)
                            .await
                        {
                            tracing::warn!(request_id = %req.request_id, "回传工具结果失败: {e}");
                        }
                    });
                }
            }
            PollOutcome::Unauthorized => {
                tracing::warn!("token 失效（401），重新登录");
                match api.relogin().await {
                    Ok(()) => {
                        backoff = BACKOFF_INIT;
                    }
                    Err(e) => {
                        tracing::warn!("重新登录失败: {e}");
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(BACKOFF_MAX);
                    }
                }
            }
            PollOutcome::Error(e) => {
                tracing::warn!("poll 失败（{}s 后重试）: {e}", backoff.as_secs());
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn req(tool: &str, input: serde_json::Value) -> ToolCallRequest {
        ToolCallRequest {
            request_id: "test-req".into(),
            tool: tool.into(),
            input,
        }
    }

    fn setup(name: &str) -> (Arc<TermManager>, NotifyTx) {
        let dir = std::env::temp_dir().join(format!("hank-cli-test-{name}"));
        // rx 直接丢弃：reader 线程对 send 失败是忽略的（let _ =），不影响被测逻辑
        let (tx, _rx) = mpsc::unbounded_channel();
        (Arc::new(TermManager::new(dir)), tx)
    }

    #[tokio::test]
    async fn unknown_tool_returns_error() {
        let (term, tx) = setup("unknown");
        let out = execute_tool(&term, &tx, &req("no_such_tool", serde_json::json!({}))).await;
        assert!(out.is_error);
        assert_eq!(out.content, "Unknown tool: no_such_tool");
    }

    #[tokio::test]
    async fn terminal_dispatch_roundtrip() {
        let (term, tx) = setup("dispatch");

        // create：空 input 走默认值 cols=120 rows=30，content 为 TermInfo JSON
        let out = execute_tool(&term, &tx, &req("terminal_create", serde_json::json!({}))).await;
        assert!(!out.is_error, "create failed: {}", out.content);
        let info: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(info["cols"], 120);
        assert_eq!(info["rows"], 30);
        let id = info["id"].as_str().unwrap().to_string();

        // list：包含刚创建的会话
        let out = execute_tool(&term, &tx, &req("terminal_list", serde_json::json!({}))).await;
        assert!(!out.is_error);
        assert!(out.content.contains(&id));

        // write → read：看到回显
        let out = execute_tool(
            &term,
            &tx,
            &req(
                "terminal_write",
                serde_json::json!({"id": id, "data": "echo dispatch-ok\n"}),
            ),
        )
        .await;
        assert!(!out.is_error);
        assert_eq!(out.content, "ok");
        let mut seen = String::new();
        for _ in 0..50 {
            let out = execute_tool(
                &term,
                &tx,
                &req("terminal_read", serde_json::json!({"id": id})),
            )
            .await;
            assert!(!out.is_error);
            if out.content.contains("dispatch-ok") {
                seen = out.content;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert!(seen.contains("dispatch-ok"), "read output: {seen}");

        // read lines 截尾：只取最后一行
        let out = execute_tool(
            &term,
            &tx,
            &req("terminal_read", serde_json::json!({"id": id, "lines": 1})),
        )
        .await;
        assert!(!out.is_error);
        assert!(
            !out.content.contains('\n'),
            "lines=1 should be single line: {:?}",
            out.content
        );

        // write 到不存在的终端：错误路径
        let out = execute_tool(
            &term,
            &tx,
            &req(
                "terminal_write",
                serde_json::json!({"id": "fake", "data": "x"}),
            ),
        )
        .await;
        assert!(out.is_error);
        assert!(
            out.content.contains("terminal not found"),
            "content: {}",
            out.content
        );

        // close
        let out = execute_tool(
            &term,
            &tx,
            &req("terminal_close", serde_json::json!({"id": id})),
        )
        .await;
        assert!(!out.is_error);
        let out = execute_tool(&term, &tx, &req("terminal_list", serde_json::json!({}))).await;
        assert_eq!(out.content, "[]");
    }

    #[tokio::test]
    async fn terminal_set_enabled_dispatch_blocks_write() {
        let (term, tx) = setup("dispatch-enabled");

        let out = execute_tool(&term, &tx, &req("terminal_create", serde_json::json!({}))).await;
        assert!(!out.is_error, "create failed: {}", out.content);
        let info: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        let id = info["id"].as_str().unwrap().to_string();

        let out = execute_tool(
            &term,
            &tx,
            &req(
                "terminal_set_enabled",
                serde_json::json!({"id": id, "enabled": false}),
            ),
        )
        .await;
        assert!(!out.is_error, "set_enabled failed: {}", out.content);
        let info: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(info["enabled"], false);

        let out = execute_tool(
            &term,
            &tx,
            &req(
                "terminal_write",
                serde_json::json!({"id": id, "data": "echo blocked\n"}),
            ),
        )
        .await;
        assert!(out.is_error);
        assert!(
            out.content.contains("terminal disabled"),
            "content: {}",
            out.content
        );

        let _ = execute_tool(
            &term,
            &tx,
            &req("terminal_close", serde_json::json!({"id": id})),
        )
        .await;
    }

    #[test]
    fn read_file_base64_roundtrip() {
        let dir = std::env::temp_dir().join("hank-cli-test-b64");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bin.dat");
        let data: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        std::fs::write(&path, &data).unwrap();
        let out = read_file_base64(path.to_str().unwrap());
        assert!(!out.is_error, "{}", out.content);
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&out.content)
            .unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn read_file_base64_rejects_dir_and_missing() {
        let out = read_file_base64("/tmp");
        assert!(out.is_error);
        assert!(
            out.content.contains("Not a regular file"),
            "{}",
            out.content
        );
        let out = read_file_base64("/tmp/hank-cli-test-no-such-file");
        assert!(out.is_error);
        assert!(
            out.content.contains("Error reading file"),
            "{}",
            out.content
        );
    }

    #[test]
    fn read_file_base64_rejects_oversize() {
        let dir = std::env::temp_dir().join("hank-cli-test-b64");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("big.bin");
        // 20MB + 1 字节
        std::fs::write(&path, vec![0u8; (MAX_READ_FILE_BASE64_BYTES + 1) as usize]).unwrap();
        let out = read_file_base64(path.to_str().unwrap());
        assert!(out.is_error);
        assert!(out.content.contains("File too large"), "{}", out.content);
        std::fs::remove_file(&path).unwrap();
    }
}
