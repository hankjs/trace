use crate::file_checksum::{compute_checksum, ChecksumStore};
use crate::{Tool, ToolOutput, ToolRisk};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::fs;

/// Precise string replacement tool — the core editing primitive for Code Agent.
/// Finds an exact substring in a file and replaces it with new content.
pub struct StrReplaceTool {
    work_dir: Option<String>,
    checksum_store: Option<ChecksumStore>,
}

impl StrReplaceTool {
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
impl Tool for StrReplaceTool {
    fn name(&self) -> &str {
        "str_replace"
    }

    fn description(&self) -> &str {
        "Replace an exact string occurrence in a file. The old_string must match exactly one \
         location in the file (including whitespace and indentation). Use this for precise edits \
         instead of rewriting entire files."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to edit (absolute or relative to work_dir)"
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact string to find and replace (must be unique in the file)"
                },
                "new_string": {
                    "type": "string",
                    "description": "The replacement string"
                }
            },
            "required": ["path", "old_string", "new_string"]
        })
    }

    fn is_write(&self) -> bool {
        true
    }

    fn risk_level(&self) -> ToolRisk {
        ToolRisk::Moderate
    }

    async fn execute(&self, input: Value) -> Result<ToolOutput> {
        let path = input["path"].as_str().unwrap_or_default();
        if path.is_empty() {
            return Ok(ToolOutput {
                content: "Error: path is required".to_string(),
                is_error: true,
            });
        }

        let old_string = input["old_string"].as_str().unwrap_or_default();
        let new_string = input["new_string"].as_str().unwrap_or_default();

        if old_string.is_empty() {
            return Ok(ToolOutput {
                content: "Error: old_string cannot be empty".to_string(),
                is_error: true,
            });
        }

        if old_string == new_string {
            return Ok(ToolOutput {
                content: "Error: old_string and new_string are identical".to_string(),
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

        // 冲突检测
        if let Some(ref store) = self.checksum_store {
            let current_checksum = compute_checksum(content.as_bytes());
            let map = store.read().await;
            if let Some(&stored_checksum) = map.get(&resolved) {
                if current_checksum != stored_checksum {
                    return Ok(ToolOutput {
                        content: format!(
                            "Error: file '{}' has been modified since last read. \
                             Please re-read the file before editing.",
                            path
                        ),
                        is_error: true,
                    });
                }
            }
        }

        // Check uniqueness: old_string must appear exactly once
        let matches: Vec<_> = content.match_indices(old_string).collect();

        if matches.is_empty() {
            // Provide helpful context: show similar lines
            let hint = find_similar_lines(&content, old_string);
            return Ok(ToolOutput {
                content: format!(
                    "Error: old_string not found in {path}. Make sure it matches exactly \
                     (including whitespace and indentation).{hint}"
                ),
                is_error: true,
            });
        }

        if matches.len() > 1 {
            return Ok(ToolOutput {
                content: format!(
                    "Error: old_string appears {} times in {path}. It must be unique. \
                     Add more surrounding context to make it unique.",
                    matches.len()
                ),
                is_error: true,
            });
        }

        // Perform the replacement
        let new_content = content.replacen(old_string, new_string, 1);

        if let Err(e) = fs::write(&resolved, &new_content).await {
            return Ok(ToolOutput {
                content: format!("Error writing file: {e}"),
                is_error: true,
            });
        }

        // 更新 checksum
        if let Some(ref store) = self.checksum_store {
            let new_checksum = compute_checksum(new_content.as_bytes());
            let mut map = store.write().await;
            map.insert(resolved.clone(), new_checksum);
        }

        // Generate a concise diff summary
        let old_lines = old_string.lines().count();
        let new_lines = new_string.lines().count();
        let line_num = content[..matches[0].0].lines().count() + 1;

        Ok(ToolOutput {
            content: format!(
                "Replaced {old_lines} lines with {new_lines} lines at line {line_num} in {path}"
            ),
            is_error: false,
        })
    }
}

/// Find lines similar to the search string to help the user fix their query
fn find_similar_lines(content: &str, search: &str) -> String {
    let search_trimmed = search.trim();
    let first_line = search_trimmed.lines().next().unwrap_or("");
    if first_line.is_empty() {
        return String::new();
    }

    // Look for lines containing the first few words
    let words: Vec<&str> = first_line.split_whitespace().take(3).collect();
    if words.is_empty() {
        return String::new();
    }

    let keyword = words[0];
    let similar: Vec<(usize, &str)> = content
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains(keyword))
        .take(3)
        .collect();

    if similar.is_empty() {
        return String::new();
    }

    let lines_str: String = similar
        .iter()
        .map(|(i, line)| format!("  L{}: {}", i + 1, line))
        .collect::<Vec<_>>()
        .join("\n");

    format!("\n\nSimilar lines found:\n{lines_str}")
}
