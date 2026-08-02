use crate::{Tool, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

/// Tool that stores generated spec and task artifacts as drafts.
pub struct GenerateArtifactsTool {
    base_url: String,
    token: String,
    change_id: String,
}

impl GenerateArtifactsTool {
    pub fn new(base_url: String, token: String, change_id: String) -> Self {
        Self {
            base_url,
            token,
            change_id,
        }
    }
}

#[async_trait]
impl Tool for GenerateArtifactsTool {
    fn name(&self) -> &str {
        "generate_artifacts"
    }

    fn description(&self) -> &str {
        "Generate change artifacts for the Spec and Task phases. Provide specs and tasks as structured output. All artifacts are saved as drafts for user review."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "specs": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "capability": { "type": "string" },
                            "content": { "type": "string" }
                        },
                        "required": ["capability", "content"]
                    },
                    "description": "Array of spec documents, each with a capability name and content"
                },
                "tasks": {
                    "type": "string",
                    "description": "The tasks markdown content (task list with groups)"
                }
            },
            "required": ["specs", "tasks"]
        })
    }

    async fn execute(&self, input: Value) -> Result<ToolOutput> {
        let client = reqwest::Client::new();
        let mut count = 0u32;

        // Helper to create an artifact
        let create = |atype: &str, capability: Option<&str>, content: &str| {
            let mut body = json!({
                "type": atype,
                "content": content,
                "status": "draft"
            });
            if let Some(cap) = capability {
                body["capability"] = json!(cap);
            }
            client
                .post(format!(
                    "{}/api/changes/{}/artifacts",
                    self.base_url, self.change_id
                ))
                .header("Authorization", format!("Bearer {}", self.token))
                .json(&body)
                .send()
        };

        // Create specs
        if let Some(specs) = input["specs"].as_array() {
            for spec in specs {
                let capability = spec["capability"].as_str().unwrap_or_default();
                let content = spec["content"].as_str().unwrap_or_default();
                if !capability.is_empty() && !content.is_empty() {
                    let resp = create("spec", Some(capability), content).await?;
                    if resp.status().is_success() {
                        count += 1;
                    }
                }
            }
        }

        // Create tasks artifact
        if let Some(tasks) = input["tasks"].as_str() {
            if !tasks.is_empty() {
                let resp = create("tasks", None, tasks).await?;
                if resp.status().is_success() {
                    count += 1;
                }
            }
        }

        Ok(ToolOutput {
            content: format!("Generated {} artifacts as drafts.", count),
            is_error: false,
        })
    }
}
