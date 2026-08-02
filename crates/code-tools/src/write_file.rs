use crate::file_checksum::{compute_checksum, ChecksumStore};
use crate::{Tool, ToolOutput, ToolRisk};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::fs;

pub struct WriteFileTool {
    work_dir: Option<String>,
    checksum_store: Option<ChecksumStore>,
}

impl WriteFileTool {
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
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates the file if it doesn't exist, or overwrites it if it does. Parent directories are created automatically."
    }

    fn is_write(&self) -> bool {
        true
    }

    fn risk_level(&self) -> ToolRisk {
        ToolRisk::Moderate
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to write (absolute or relative to work_dir)"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "required": ["path", "content"]
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

        let content = input["content"].as_str().unwrap_or_default();
        let resolved = self.resolve_path(path);

        // 冲突检测: 如果文件已存在且有 checksum 记录，验证是否被外部修改
        if let Some(ref store) = self.checksum_store {
            if let Ok(current_content) = fs::read_to_string(&resolved).await {
                let current_checksum = compute_checksum(current_content.as_bytes());
                let map = store.read().await;
                if let Some(&stored_checksum) = map.get(&resolved) {
                    if current_checksum != stored_checksum {
                        return Ok(ToolOutput {
                            content: format!(
                                "Error: file '{}' has been modified since last read. \
                                 Please re-read the file before writing.",
                                path
                            ),
                            is_error: true,
                        });
                    }
                }
            }
        }

        // Create parent directories if needed
        if let Some(parent) = std::path::Path::new(&resolved).parent() {
            if let Err(e) = fs::create_dir_all(parent).await {
                return Ok(ToolOutput {
                    content: format!("Error creating directories: {e}"),
                    is_error: true,
                });
            }
        }

        match fs::write(&resolved, content).await {
            Ok(()) => {
                // 更新 checksum store
                if let Some(ref store) = self.checksum_store {
                    let new_checksum = compute_checksum(content.as_bytes());
                    let mut map = store.write().await;
                    map.insert(resolved.clone(), new_checksum);
                }
                let lines = content.lines().count();
                Ok(ToolOutput {
                    content: format!("Successfully wrote {lines} lines to {path}"),
                    is_error: false,
                })
            }
            Err(e) => Ok(ToolOutput {
                content: format!("Error writing file: {e}"),
                is_error: true,
            }),
        }
    }
}
