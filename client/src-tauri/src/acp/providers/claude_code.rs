use async_trait::async_trait;
use serde_json::Value;
use tokio::io::AsyncBufReadExt;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing;

use crate::acp::events::AcpEvent;
use crate::acp::provider::{CliProvider, ProviderInfo, ProviderSession};

pub struct ClaudeCodeProvider;

impl ClaudeCodeProvider {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl CliProvider for ClaudeCodeProvider {
    fn name(&self) -> &str {
        "claude-code"
    }

    async fn test(&self, binary_path: &str, _work_dir: &str) -> Result<ProviderInfo, String> {
        let output = Command::new(binary_path)
            .arg("--version")
            .output()
            .await
            .map_err(|e| format!("Failed to run '{}': {}", binary_path, e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Version check failed: {}", stderr.trim()));
        }

        let version_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(ProviderInfo {
            version: Some(version_str),
            model: None,
        })
    }

    async fn prompt(
        &self,
        binary_path: &str,
        message: &str,
        session: &mut ProviderSession,
        event_tx: mpsc::Sender<AcpEvent>,
        cancel_token: CancellationToken,
    ) -> Result<(), String> {
        let mut cmd = Command::new(binary_path);
        cmd.arg("-p")
            .arg(message)
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose");

        if let Some(ref sid) = session.cli_session_id {
            cmd.arg("--resume").arg(sid);
        }

        cmd.current_dir(&session.work_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn claude: {}", e))?;

        let stdout = child.stdout.take().ok_or("No stdout from claude process")?;

        let event_tx_clone = event_tx.clone();
        let cancel = cancel_token.clone();

        // Spawn a task to kill the process on cancellation
        let child_id = child.id();
        let cancel_for_kill = cancel_token.clone();
        tokio::spawn(async move {
            cancel_for_kill.cancelled().await;
            if let Some(pid) = child_id {
                unsafe {
                    libc::kill(pid as i32, libc::SIGTERM);
                }
            }
        });

        // Read stdout line by line
        let reader = tokio::io::BufReader::new(stdout);
        let mut lines = reader.lines();

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    let _ = event_tx_clone.send(AcpEvent::Error {
                        message: "Cancelled".to_string(),
                    }).await;
                    break;
                }
                line_result = lines.next_line() => {
                    match line_result {
                        Ok(Some(line)) => {
                            if line.trim().is_empty() {
                                continue;
                            }
                            if let Err(e) = parse_claude_line(&line, session, &event_tx_clone).await {
                                tracing::warn!("Failed to parse claude output: {}", e);
                            }
                        }
                        Ok(None) => break, // EOF
                        Err(e) => {
                            let _ = event_tx_clone.send(AcpEvent::Error {
                                message: format!("Read error: {}", e),
                            }).await;
                            break;
                        }
                    }
                }
            }
        }

        // Wait for process to finish
        let _ = child.wait().await;
        Ok(())
    }
}

async fn parse_claude_line(
    line: &str,
    session: &mut ProviderSession,
    event_tx: &mpsc::Sender<AcpEvent>,
) -> Result<(), String> {
    let v: Value = serde_json::from_str(line).map_err(|e| format!("JSON parse error: {}", e))?;

    let msg_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let subtype = v.get("subtype").and_then(|t| t.as_str()).unwrap_or("");

    match msg_type {
        "system" => {
            // Extract session_id from init message for future --resume
            if subtype == "init" {
                if let Some(sid) = v.get("session_id").and_then(|s| s.as_str()) {
                    session.cli_session_id = Some(sid.to_string());
                }
            }
        }
        "assistant" => {
            // Content blocks from assistant message
            if let Some(message) = v.get("message") {
                if let Some(content) = message.get("content").and_then(|c| c.as_array()) {
                    for block in content {
                        let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        match block_type {
                            "text" => {
                                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                    let _ = event_tx
                                        .send(AcpEvent::TextDelta {
                                            content: text.to_string(),
                                        })
                                        .await;
                                }
                            }
                            "tool_use" => {
                                let id = block
                                    .get("id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let name = block
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let input = block
                                    .get("input")
                                    .cloned()
                                    .unwrap_or(Value::Object(Default::default()));
                                let _ = event_tx
                                    .send(AcpEvent::ToolUse {
                                        tool_call_id: id,
                                        tool_name: name,
                                        input,
                                    })
                                    .await;
                            }
                            "tool_result" => {
                                let tool_use_id = block
                                    .get("tool_use_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let content = block.get("content").cloned().unwrap_or(Value::Null);
                                let is_error = block
                                    .get("is_error")
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(false);
                                let _ = event_tx
                                    .send(AcpEvent::ToolResult {
                                        tool_call_id: tool_use_id,
                                        output: content,
                                        is_error,
                                    })
                                    .await;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        "result" => match subtype {
            "success" => {
                let _ = event_tx
                    .send(AcpEvent::Done {
                        stop_reason: "end_turn".to_string(),
                    })
                    .await;
            }
            "error" => {
                let error_msg = v
                    .get("error")
                    .and_then(|e| e.as_str())
                    .unwrap_or("Unknown error")
                    .to_string();
                let _ = event_tx.send(AcpEvent::Error { message: error_msg }).await;
            }
            _ => {}
        },
        _ => {}
    }

    Ok(())
}
