use crate::{Tool, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

const MAX_ENTRIES: usize = 200;
const TIMEOUT_SECS: u64 = 15;

/// List directory contents with optional glob pattern matching.
pub struct ListDirectoryTool {
    work_dir: Option<String>,
}

impl ListDirectoryTool {
    pub fn new(work_dir: Option<String>) -> Self {
        Self { work_dir }
    }

    fn resolve_path(&self, path: &str) -> String {
        if path.starts_with('/') {
            return path.to_string();
        }
        match &self.work_dir {
            Some(dir) => format!("{}/{}", dir.trim_end_matches('/'), path),
            None => path.to_string(),
        }
    }
}

#[async_trait]
impl Tool for ListDirectoryTool {
    fn name(&self) -> &str {
        "list_directory"
    }

    fn description(&self) -> &str {
        "List files and directories. Without a glob pattern, lists the immediate contents \
         of the directory. With a glob pattern (e.g. '**/*.rs'), finds all matching files \
         recursively. Uses 'fd' or falls back to 'find'."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory path to list (default: work_dir)"
                },
                "glob": {
                    "type": "string",
                    "description": "Glob pattern to match files, e.g. '**/*.rs', '*.ts'"
                }
            },
            "required": []
        })
    }

    async fn execute(&self, input: Value) -> Result<ToolOutput> {
        let path = input["path"].as_str().unwrap_or(".");
        let resolved = self.resolve_path(path);

        if !Path::new(&resolved).exists() {
            return Ok(ToolOutput {
                content: format!("Error: path does not exist: {resolved}"),
                is_error: true,
            });
        }

        let glob_pattern = input["glob"].as_str();

        let output = if let Some(pattern) = glob_pattern {
            self.glob_search(&resolved, pattern).await
        } else {
            self.list_dir(&resolved).await
        };

        output
    }
}

impl ListDirectoryTool {
    async fn list_dir(&self, path: &str) -> Result<ToolOutput> {
        let mut cmd = Command::new("ls");
        cmd.arg("-la").arg(path);

        let result = tokio::time::timeout(Duration::from_secs(TIMEOUT_SECS), cmd.output()).await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let lines: Vec<&str> = stdout.lines().collect();
                let total = lines.len();

                let display = if total > MAX_ENTRIES {
                    format!(
                        "{}\n\n... ({total} entries, showing first {MAX_ENTRIES})",
                        lines[..MAX_ENTRIES].join("\n")
                    )
                } else {
                    lines.join("\n")
                };

                Ok(ToolOutput {
                    content: display,
                    is_error: false,
                })
            }
            Ok(Err(e)) => Ok(ToolOutput {
                content: format!("Error listing directory: {e}"),
                is_error: true,
            }),
            Err(_) => Ok(ToolOutput {
                content: format!("Error: listing timed out after {TIMEOUT_SECS}s"),
                is_error: true,
            }),
        }
    }

    async fn glob_search(&self, path: &str, pattern: &str) -> Result<ToolOutput> {
        // Try fd first, fall back to find
        let result = self.try_fd(path, pattern).await;
        match result {
            Ok(output) if !output.is_error => Ok(output),
            _ => self.try_find(path, pattern).await,
        }
    }

    async fn try_fd(&self, path: &str, pattern: &str) -> Result<ToolOutput> {
        let mut cmd = Command::new("fd");
        cmd.arg("--glob")
            .arg(pattern)
            .arg(path)
            .arg("--color=never");

        let result = tokio::time::timeout(Duration::from_secs(TIMEOUT_SECS), cmd.output()).await;

        match result {
            Ok(Ok(output)) => {
                if !output.status.success() && output.stdout.is_empty() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Ok(ToolOutput {
                        content: format!("fd error: {stderr}"),
                        is_error: true,
                    });
                }
                let stdout = String::from_utf8_lossy(&output.stdout);
                Ok(self.format_file_list(&stdout))
            }
            Ok(Err(_)) => Ok(ToolOutput {
                content: "fd not available".to_string(),
                is_error: true,
            }),
            Err(_) => Ok(ToolOutput {
                content: format!("Error: glob search timed out after {TIMEOUT_SECS}s"),
                is_error: true,
            }),
        }
    }

    async fn try_find(&self, path: &str, pattern: &str) -> Result<ToolOutput> {
        let mut cmd = Command::new("find");
        cmd.arg(path)
            .arg("-name")
            .arg(pattern)
            .arg("-type")
            .arg("f");

        let result = tokio::time::timeout(Duration::from_secs(TIMEOUT_SECS), cmd.output()).await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                Ok(self.format_file_list(&stdout))
            }
            Ok(Err(e)) => Ok(ToolOutput {
                content: format!("Error running find: {e}"),
                is_error: true,
            }),
            Err(_) => Ok(ToolOutput {
                content: format!("Error: find timed out after {TIMEOUT_SECS}s"),
                is_error: true,
            }),
        }
    }

    fn format_file_list(&self, raw: &str) -> ToolOutput {
        let lines: Vec<&str> = raw.lines().filter(|l| !l.is_empty()).collect();
        let total = lines.len();

        if total == 0 {
            return ToolOutput {
                content: "No files found matching the pattern.".to_string(),
                is_error: false,
            };
        }

        let display = if total > MAX_ENTRIES {
            format!(
                "{}\n\n... ({total} files, showing first {MAX_ENTRIES})",
                lines[..MAX_ENTRIES].join("\n")
            )
        } else {
            format!("{}\n\n({total} files)", lines.join("\n"))
        };

        ToolOutput {
            content: display,
            is_error: false,
        }
    }
}
