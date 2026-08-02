use crate::{Tool, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

/// Tool that allows the agent to ask the user a question with options.
/// When called, the agent loop should detect this tool and interrupt execution.
pub struct AskUserTool;

impl AskUserTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn description(&self) -> &str {
        "Ask the user a question with multiple choice options. The agent loop will pause and wait for the user's response before continuing."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask the user"
                },
                "options": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "List of options for the user to choose from"
                }
            },
            "required": ["question", "options"]
        })
    }

    async fn execute(&self, input: Value) -> Result<ToolOutput> {
        // This tool's execution is a no-op — the actual interruption logic
        // is handled by the agent loop in session.rs which detects the tool name.
        let question = input["question"].as_str().unwrap_or_default();
        let options = input["options"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        Ok(ToolOutput {
            content: format!("Asked user: {} [{}]", question, options),
            is_error: false,
        })
    }
}
