use crate::{Tool, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::process::Command;

const MAX_RESULTS: usize = 50;
const TIMEOUT_SECS: u64 = 15;

pub struct SearchTool {
    work_dir: Option<String>,
}

impl SearchTool {
    pub fn new(work_dir: Option<String>) -> Self {
        Self { work_dir }
    }
}

#[async_trait]
impl Tool for SearchTool {
    fn name(&self) -> &str {
        "search"
    }

    fn description(&self) -> &str {
        "Search for text patterns in files using ripgrep (rg). Supports regex patterns and file type filtering. Returns matching lines with file paths and line numbers."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to search in (default: work_dir)"
                },
                "glob": {
                    "type": "string",
                    "description": "File glob pattern to filter, e.g. '*.rs', '*.{ts,tsx}'"
                },
                "ignore_case": {
                    "type": "boolean",
                    "description": "Case insensitive search (default: false)"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, input: Value) -> Result<ToolOutput> {
        let pattern = input["pattern"].as_str().unwrap_or_default();
        if pattern.is_empty() {
            return Ok(ToolOutput {
                content: "Error: pattern is required".to_string(),
                is_error: true,
            });
        }

        let search_path = input["path"]
            .as_str()
            .map(|p| {
                if p.starts_with('/') {
                    p.to_string()
                } else {
                    match &self.work_dir {
                        Some(dir) => format!("{}/{}", dir.trim_end_matches('/'), p),
                        None => p.to_string(),
                    }
                }
            })
            .or_else(|| self.work_dir.clone())
            .unwrap_or_else(|| ".".to_string());

        let mut cmd = Command::new("rg");
        cmd.arg("--line-number")
            .arg("--no-heading")
            .arg("--color=never")
            .arg(format!("--max-count={MAX_RESULTS}"));

        if input["ignore_case"].as_bool().unwrap_or(false) {
            cmd.arg("--ignore-case");
        }

        if let Some(glob) = input["glob"].as_str() {
            cmd.arg("--glob").arg(glob);
        }

        cmd.arg(pattern).arg(&search_path);

        let result = tokio::time::timeout(Duration::from_secs(TIMEOUT_SECS), cmd.output()).await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                if output.status.code() == Some(1) && stdout.is_empty() {
                    return Ok(ToolOutput {
                        content: "No matches found.".to_string(),
                        is_error: false,
                    });
                }

                if !output.status.success() && output.status.code() != Some(1) {
                    return Ok(ToolOutput {
                        content: format!("Search error: {stderr}"),
                        is_error: true,
                    });
                }

                let lines: Vec<&str> = stdout.lines().collect();
                let total = lines.len();
                let display: String = lines
                    .into_iter()
                    .take(MAX_RESULTS)
                    .collect::<Vec<_>>()
                    .join("\n");

                let summary = if total > MAX_RESULTS {
                    format!("{display}\n\n... ({total} matches, showing first {MAX_RESULTS})")
                } else {
                    format!("{display}\n\n({total} matches)")
                };

                Ok(ToolOutput {
                    content: summary,
                    is_error: false,
                })
            }
            Ok(Err(e)) => Ok(ToolOutput {
                content: format!(
                    "Error running search: {e}. Make sure 'rg' (ripgrep) is installed."
                ),
                is_error: true,
            }),
            Err(_) => Ok(ToolOutput {
                content: format!("Error: search timed out after {TIMEOUT_SECS}s"),
                is_error: true,
            }),
        }
    }
}
