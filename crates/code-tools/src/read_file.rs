use crate::file_checksum::{compute_checksum, ChecksumStore};
use crate::{Tool, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::fs;

const MAX_READ_BYTES: usize = 200_000;

pub struct ReadFileTool {
    work_dir: Option<String>,
    checksum_store: Option<ChecksumStore>,
}

impl ReadFileTool {
    pub fn new(work_dir: Option<String>) -> Self {
        Self {
            work_dir,
            checksum_store: None,
        }
    }

    pub fn with_checksum_store(work_dir: Option<String>, store: ChecksumStore) -> Self {
        Self {
            work_dir,
            checksum_store: Some(store),
        }
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
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Returns the file content as text. Supports an optional offset and limit (line numbers, 1-based)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to read (absolute or relative to work_dir)"
                },
                "offset": {
                    "type": "integer",
                    "description": "Start reading from this line number (1-based, optional)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read (optional)"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, input: Value) -> Result<ToolOutput> {
        let path = input["path"].as_str().unwrap_or_default();
        if path.is_empty() {
            return Ok(ToolOutput {
                content: "Error: path is required".to_string(),
                is_error: true,
            });
        }

        let resolved = self.resolve_path(path);

        let content = match fs::read_to_string(&resolved).await {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolOutput {
                    content: format!("Error reading file: {e}"),
                    is_error: true,
                });
            }
        };

        // 存储 checksum 用于冲突检测
        if let Some(ref store) = self.checksum_store {
            let checksum = compute_checksum(content.as_bytes());
            let mut map = store.write().await;
            map.insert(resolved.clone(), checksum);
        }

        let offset = input["offset"].as_u64().unwrap_or(1).max(1) as usize;
        let limit = input["limit"].as_u64().map(|l| l as usize);

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();
        let start = (offset - 1).min(total_lines);
        let end = match limit {
            Some(l) => (start + l).min(total_lines),
            None => total_lines,
        };

        let selected: String = lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>4}\t{}", start + i + 1, line))
            .collect::<Vec<_>>()
            .join("\n");

        let output = if selected.len() > MAX_READ_BYTES {
            format!(
                "{}\n\n... [truncated, file has {} lines total]",
                &selected[..MAX_READ_BYTES],
                total_lines
            )
        } else {
            format!("{selected}\n\n({total_lines} lines total)")
        };

        Ok(ToolOutput {
            content: output,
            is_error: false,
        })
    }
}
