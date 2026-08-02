use async_trait::async_trait;
use serde_json::Value;
use tokio::io::AsyncBufReadExt;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing;

use crate::acp::events::AcpEvent;
use crate::acp::provider::{CliProvider, ProviderInfo, ProviderSession};

pub struct CodexProvider;

impl CodexProvider {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl CliProvider for CodexProvider {
    fn name(&self) -> &str {
        "codex"
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
        cmd.arg("exec")
            .arg(message)
            .arg("--json")
            .arg("--ephemeral")
            .arg("-C")
            .arg(&session.work_dir)
            .arg("-s")
            .arg("danger-full-access");

        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn codex: {}", e))?;

        let stdout = child.stdout.take().ok_or("No stdout from codex process")?;

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
                            if let Err(e) = parse_codex_line(&line, session, &event_tx_clone).await {
                                tracing::warn!("Failed to parse codex output: {}", e);
                            }
                        }
                        Ok(None) => break,
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

        let _ = child.wait().await;
        Ok(())
    }
}

async fn parse_codex_line(
    line: &str,
    session: &mut ProviderSession,
    event_tx: &mpsc::Sender<AcpEvent>,
) -> Result<(), String> {
    let v: Value = serde_json::from_str(line).map_err(|e| format!("JSON parse error: {}", e))?;

    let event_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match event_type {
        "thread.started" => {
            // Store thread_id for potential future use
            if let Some(tid) = v.get("thread_id").and_then(|t| t.as_str()) {
                session.cli_session_id = Some(tid.to_string());
            }
        }
        "turn.started" => {
            // No event to emit
        }
        "message.delta" => {
            if let Some(delta) = v.get("delta") {
                if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                    let _ = event_tx
                        .send(AcpEvent::TextDelta {
                            content: content.to_string(),
                        })
                        .await;
                }
            }
        }
        "tool_call.started" => {
            let id = v
                .get("tool_call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = v
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let input = v
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
        "tool_call.completed" => {
            let id = v
                .get("tool_call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let output = v.get("output").cloned().unwrap_or(Value::Null);
            let is_error = v.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
            let _ = event_tx
                .send(AcpEvent::ToolResult {
                    tool_call_id: id,
                    output,
                    is_error,
                })
                .await;
        }
        "turn.completed" => {
            let _ = event_tx
                .send(AcpEvent::Done {
                    stop_reason: "end_turn".to_string(),
                })
                .await;
        }
        "error" => {
            let message = v
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error")
                .to_string();
            let _ = event_tx.send(AcpEvent::Error { message }).await;
        }
        _ => {}
    }

    Ok(())
}
