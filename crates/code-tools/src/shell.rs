use crate::{Tool, ToolOutput, ToolRisk, DEFAULT_BLOCKED_COMMANDS, LONG_TOOL_TIMEOUT};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::warn;

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_OUTPUT_BYTES: usize = 100_000;

pub struct ShellTool {
    work_dir: Option<String>,
    run_as_user: Option<String>,
}

impl ShellTool {
    pub fn new(work_dir: Option<String>) -> Self {
        Self {
            work_dir,
            run_as_user: None,
        }
    }

    pub fn new_as_user(work_dir: Option<String>, user: impl Into<String>) -> Self {
        Self {
            work_dir,
            run_as_user: Some(user.into()),
        }
    }

    fn shell_command(&self) -> Command {
        if let Some(user) = &self.run_as_user {
            let mut cmd = Command::new("sudo");
            let home = format!("HOME=/home/{user}");
            let path = format!(
                "PATH=/home/{user}/.cargo/bin:/home/{user}/.local/bin:/usr/local/bin:/usr/bin:/bin"
            );
            cmd.args([
                "--non-interactive",
                "--user",
                user,
                "--",
                "env",
                &home,
                &path,
                "sh",
            ]);
            cmd
        } else {
            Command::new("sh")
        }
    }

    fn is_blocked(command: &str) -> bool {
        let lower = command.to_lowercase();
        DEFAULT_BLOCKED_COMMANDS
            .iter()
            .any(|b| lower.contains(&b.to_lowercase()))
    }

    fn truncate_output(output: &str) -> String {
        if output.len() <= MAX_OUTPUT_BYTES {
            return output.to_string();
        }
        let half = MAX_OUTPUT_BYTES / 2;
        let start = &output[..half];
        let end = &output[output.len() - half..];
        format!(
            "{start}\n\n... [output truncated, {len} bytes total] ...\n\n{end}",
            len = output.len()
        )
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute a shell command and return its output. Commands have a timeout and output size limit."
    }

    fn timeout(&self) -> Duration {
        LONG_TOOL_TIMEOUT
    }

    fn is_write(&self) -> bool {
        true
    }

    fn risk_level(&self) -> ToolRisk {
        ToolRisk::Dangerous
    }

    fn supports_streaming(&self) -> bool {
        true
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Timeout in seconds (default 30)"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, input: Value) -> Result<ToolOutput> {
        let command = input["command"].as_str().unwrap_or_default().to_string();

        if command.is_empty() {
            return Ok(ToolOutput {
                content: "Error: command is required".to_string(),
                is_error: true,
            });
        }

        if Self::is_blocked(&command) {
            warn!("Blocked dangerous command: {command}");
            return Ok(ToolOutput {
                content: "Error: this command is blocked for safety reasons".to_string(),
                is_error: true,
            });
        }

        let timeout_secs = input["timeout_secs"]
            .as_u64()
            .unwrap_or(DEFAULT_TIMEOUT_SECS);

        let result = tokio::time::timeout(Duration::from_secs(timeout_secs), {
            let mut cmd = self.shell_command();
            cmd.arg("-c").arg(&command);
            if let Some(ref dir) = self.work_dir {
                cmd.current_dir(dir);
            }
            cmd.output()
        })
        .await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let code = output.status.code().unwrap_or(-1);

                let combined = if stderr.is_empty() {
                    format!("Exit code: {code}\n{stdout}")
                } else {
                    format!("Exit code: {code}\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}")
                };

                Ok(ToolOutput {
                    content: Self::truncate_output(&combined),
                    is_error: code != 0,
                })
            }
            Ok(Err(e)) => Ok(ToolOutput {
                content: format!("Error executing command: {e}"),
                is_error: true,
            }),
            Err(_) => Ok(ToolOutput {
                content: format!("Error: command timed out after {timeout_secs}s"),
                is_error: true,
            }),
        }
    }

    async fn execute_streaming(
        &self,
        input: Value,
        stream_tx: mpsc::Sender<String>,
    ) -> Result<ToolOutput> {
        let command = input["command"].as_str().unwrap_or_default().to_string();

        if command.is_empty() {
            return Ok(ToolOutput {
                content: "Error: command is required".to_string(),
                is_error: true,
            });
        }

        if Self::is_blocked(&command) {
            warn!("Blocked dangerous command: {command}");
            return Ok(ToolOutput {
                content: "Error: this command is blocked for safety reasons".to_string(),
                is_error: true,
            });
        }

        let timeout_secs = input["timeout_secs"]
            .as_u64()
            .unwrap_or(DEFAULT_TIMEOUT_SECS);

        let mut cmd = self.shell_command();
        cmd.arg("-c").arg(&command);
        if let Some(ref dir) = self.work_dir {
            cmd.current_dir(dir);
        }
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolOutput {
                    content: format!("Error spawning command: {e}"),
                    is_error: true,
                });
            }
        };

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut stderr_reader = BufReader::new(stderr).lines();

        let mut full_output = String::new();

        let result = tokio::time::timeout(Duration::from_secs(timeout_secs), async {
            loop {
                tokio::select! {
                    line = stdout_reader.next_line() => {
                        match line {
                            Ok(Some(l)) => {
                                let _ = stream_tx.send(l.clone()).await;
                                full_output.push_str(&l);
                                full_output.push('\n');
                            }
                            Ok(None) => break,
                            Err(_) => break,
                        }
                    }
                    line = stderr_reader.next_line() => {
                        match line {
                            Ok(Some(l)) => {
                                let _ = stream_tx.send(format!("[stderr] {l}")).await;
                                full_output.push_str(&l);
                                full_output.push('\n');
                            }
                            Ok(None) => {}
                            Err(_) => {}
                        }
                    }
                }
            }
            child.wait().await
        })
        .await;

        match result {
            Ok(Ok(status)) => {
                let code = status.code().unwrap_or(-1);
                let content = format!("Exit code: {code}\n{}", Self::truncate_output(&full_output));
                Ok(ToolOutput {
                    content,
                    is_error: code != 0,
                })
            }
            Ok(Err(e)) => Ok(ToolOutput {
                content: format!("Error waiting for command: {e}"),
                is_error: true,
            }),
            Err(_) => {
                let _ = child.kill().await;
                Ok(ToolOutput {
                    content: format!("Error: command timed out after {timeout_secs}s"),
                    is_error: true,
                })
            }
        }
    }
}
