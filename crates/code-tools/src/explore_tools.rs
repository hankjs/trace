use crate::{Tool, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

/// Tool that signals the explore phase is complete.
/// Stores the summary on the change record. If no change exists yet, creates one.
pub struct FinalizeExploreTool {
    base_url: String,
    token: String,
    change_id: String,
    session_id: String,
}

impl FinalizeExploreTool {
    pub fn new(base_url: String, token: String, change_id: String, session_id: String) -> Self {
        Self {
            base_url,
            token,
            change_id,
            session_id,
        }
    }
}

#[async_trait]
impl Tool for FinalizeExploreTool {
    fn name(&self) -> &str {
        "finalize_explore"
    }

    fn description(&self) -> &str {
        "Signal that the explore phase is complete. Provide a comprehensive summary and a short name for the change."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "A comprehensive summary of the requirements gathered during exploration"
                },
                "name": {
                    "type": "string",
                    "description": "A short name/title for this change (e.g. 'Add user authentication')"
                }
            },
            "required": ["summary", "name"]
        })
    }

    async fn execute(&self, input: Value) -> Result<ToolOutput> {
        let summary = input["summary"].as_str().unwrap_or_default();
        let name = input["name"].as_str().unwrap_or_default();
        if summary.is_empty() {
            return Ok(ToolOutput {
                content: "summary is required".to_string(),
                is_error: true,
            });
        }
        if name.is_empty() {
            return Ok(ToolOutput {
                content: "name is required".to_string(),
                is_error: true,
            });
        }

        let client = reqwest::Client::new();

        // Determine change_id: use existing or create a new change
        let change_id = if self.change_id.is_empty() {
            // Create a new change via API
            let resp = client
                .post(format!("{}/api/changes", self.base_url))
                .header("Authorization", format!("Bearer {}", self.token))
                .json(&json!({ "name": name }))
                .send()
                .await?;
            if !resp.status().is_success() {
                return Ok(ToolOutput {
                    content: "Failed to create change".to_string(),
                    is_error: true,
                });
            }
            let body: Value = resp.json().await?;
            let cid = body["data"]["id"].as_str().unwrap_or_default().to_string();
            if cid.is_empty() {
                return Ok(ToolOutput {
                    content: "Failed to get change id from response".to_string(),
                    is_error: true,
                });
            }

            // Bind the change to this session
            let _ = client
                .put(format!(
                    "{}/api/sessions/{}",
                    self.base_url, self.session_id
                ))
                .header("Authorization", format!("Bearer {}", self.token))
                .json(&json!({ "change_id": cid }))
                .send()
                .await;

            cid
        } else {
            self.change_id.clone()
        };

        // Update the change's explore_summary via API
        let resp = client
            .put(format!("{}/api/changes/{}", self.base_url, change_id))
            .header("Authorization", format!("Bearer {}", self.token))
            .json(&json!({ "explore_summary": summary, "name": name }))
            .send()
            .await?;

        if resp.status().is_success() {
            Ok(ToolOutput {
                content: format!(
                    "Explore phase finalized successfully. Change '{}' created.",
                    name
                ),
                is_error: false,
            })
        } else {
            Ok(ToolOutput {
                content: "Failed to finalize explore phase".to_string(),
                is_error: true,
            })
        }
    }
}
