use crate::{Tool, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct UpdateSpecTool {
    base_url: String,
    token: String,
    session_id: String,
}

impl UpdateSpecTool {
    pub fn new(base_url: String, token: String, session_id: String) -> Self {
        Self {
            base_url,
            token,
            session_id,
        }
    }
}

#[async_trait]
impl Tool for UpdateSpecTool {
    fn name(&self) -> &str {
        "update_spec"
    }

    fn description(&self) -> &str {
        "Update a main spec's content by capability name. Increments version and stores a snapshot of the previous version."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "capability": {
                    "type": "string",
                    "description": "The capability name of the spec to update"
                },
                "content": {
                    "type": "string",
                    "description": "The new markdown content for the spec"
                },
                "reason": {
                    "type": "string",
                    "description": "Brief reason for the update"
                }
            },
            "required": ["capability", "content"]
        })
    }

    async fn execute(&self, input: Value) -> Result<ToolOutput> {
        let capability = input["capability"].as_str().unwrap_or_default();
        let content = input["content"].as_str().unwrap_or_default();

        if capability.is_empty() || content.is_empty() {
            return Ok(ToolOutput {
                content: "capability and content are required".to_string(),
                is_error: true,
            });
        }

        let client = reqwest::Client::new();
        let list_resp = client
            .get(format!("{}/api/specs", self.base_url))
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await?;

        if !list_resp.status().is_success() {
            return Ok(ToolOutput {
                content: "failed to list specs".to_string(),
                is_error: true,
            });
        }

        let body: Value = list_resp.json().await?;
        let empty = vec![];
        let specs = body["data"].as_array().unwrap_or(&empty);
        let spec = specs
            .iter()
            .find(|s| s["capability"].as_str() == Some(capability));

        let spec_id = match spec {
            Some(s) => s["id"].as_str().unwrap_or_default().to_string(),
            None => {
                return Ok(ToolOutput {
                    content: format!("spec not found: {}", capability),
                    is_error: true,
                })
            }
        };

        let update_resp = client
            .put(format!("{}/api/specs/{}", self.base_url, spec_id))
            .header("Authorization", format!("Bearer {}", self.token))
            .header("X-Session-Id", &self.session_id)
            .json(&json!({ "content": content }))
            .send()
            .await?;

        if update_resp.status().is_success() {
            Ok(ToolOutput {
                content: format!("Updated spec '{}' successfully", capability),
                is_error: false,
            })
        } else {
            let err_body: Value = update_resp.json().await.unwrap_or_default();
            Ok(ToolOutput {
                content: format!("Failed to update spec: {}", err_body["msg"]),
                is_error: true,
            })
        }
    }
}

// ─── UpdateTaskStatusTool ────────────────────────────────────────────

pub struct UpdateTaskStatusTool {
    base_url: String,
    token: String,
    session_id: String,
}

impl UpdateTaskStatusTool {
    pub fn new(base_url: String, token: String, session_id: String) -> Self {
        Self {
            base_url,
            token,
            session_id,
        }
    }
}

#[async_trait]
impl Tool for UpdateTaskStatusTool {
    fn name(&self) -> &str {
        "update_task_status"
    }

    fn description(&self) -> &str {
        "Update a change task's status. Valid statuses: pending, in_progress, done."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "change_id": {
                    "type": "string",
                    "description": "The change ID"
                },
                "task_id": {
                    "type": "string",
                    "description": "The task ID to update"
                },
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "done"],
                    "description": "The new status for the task"
                }
            },
            "required": ["change_id", "task_id", "status"]
        })
    }

    async fn execute(&self, input: Value) -> Result<ToolOutput> {
        let change_id = input["change_id"].as_str().unwrap_or_default();
        let task_id = input["task_id"].as_str().unwrap_or_default();
        let status = input["status"].as_str().unwrap_or_default();

        if change_id.is_empty() || task_id.is_empty() || status.is_empty() {
            return Ok(ToolOutput {
                content: "change_id, task_id, and status are required".to_string(),
                is_error: true,
            });
        }

        let client = reqwest::Client::new();
        let resp = client
            .put(format!(
                "{}/api/changes/{}/tasks/{}",
                self.base_url, change_id, task_id
            ))
            .header("Authorization", format!("Bearer {}", self.token))
            .header("X-Session-Id", &self.session_id)
            .json(&json!({ "status": status }))
            .send()
            .await?;

        if resp.status().is_success() {
            Ok(ToolOutput {
                content: format!("Task status updated to '{}'", status),
                is_error: false,
            })
        } else {
            let err_body: Value = resp.json().await.unwrap_or_default();
            Ok(ToolOutput {
                content: format!("Failed to update task: {}", err_body["msg"]),
                is_error: true,
            })
        }
    }
}

// ─── UpdateArtifactTool ──────────────────────────────────────────────

pub struct UpdateArtifactTool {
    base_url: String,
    token: String,
    session_id: String,
}

impl UpdateArtifactTool {
    pub fn new(base_url: String, token: String, session_id: String) -> Self {
        Self {
            base_url,
            token,
            session_id,
        }
    }
}

#[async_trait]
impl Tool for UpdateArtifactTool {
    fn name(&self) -> &str {
        "update_artifact"
    }

    fn description(&self) -> &str {
        "Update a change artifact's content and/or metadata."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "change_id": {
                    "type": "string",
                    "description": "The change ID"
                },
                "artifact_id": {
                    "type": "string",
                    "description": "The artifact ID to update"
                },
                "content": {
                    "type": "string",
                    "description": "New markdown content for the artifact"
                },
                "metadata": {
                    "type": "object",
                    "description": "Optional metadata JSON to update"
                }
            },
            "required": ["change_id", "artifact_id", "content"]
        })
    }

    async fn execute(&self, input: Value) -> Result<ToolOutput> {
        let change_id = input["change_id"].as_str().unwrap_or_default();
        let artifact_id = input["artifact_id"].as_str().unwrap_or_default();
        let content = input["content"].as_str().unwrap_or_default();
        let metadata = input.get("metadata");

        if change_id.is_empty() || artifact_id.is_empty() || content.is_empty() {
            return Ok(ToolOutput {
                content: "change_id, artifact_id, and content are required".to_string(),
                is_error: true,
            });
        }

        let mut body = json!({ "content": content });
        if let Some(m) = metadata {
            body["metadata"] = m.clone();
        }

        let client = reqwest::Client::new();
        let resp = client
            .put(format!(
                "{}/api/changes/{}/artifacts/{}",
                self.base_url, change_id, artifact_id
            ))
            .header("Authorization", format!("Bearer {}", self.token))
            .header("X-Session-Id", &self.session_id)
            .json(&body)
            .send()
            .await?;

        if resp.status().is_success() {
            Ok(ToolOutput {
                content: "Artifact updated successfully".to_string(),
                is_error: false,
            })
        } else {
            let err_body: Value = resp.json().await.unwrap_or_default();
            Ok(ToolOutput {
                content: format!("Failed to update artifact: {}", err_body["msg"]),
                is_error: true,
            })
        }
    }
}
