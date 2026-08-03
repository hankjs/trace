//! agent 在收尾时提议「下一步可以做什么」，渠道渲染成可点按钮。
//!
//! 与 ask_user 的区别：ask_user 会**中断** agent 循环等用户回答；
//! suggest_actions 不中断，只记录建议，agent 继续跑到结束。
//! 所以它不需要 tool_use_id 配对的 resume 逻辑。

use crate::{Tool, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

/// Tool that proposes follow-up actions after finishing the current task.
/// When called, the agent loop records suggestions and continues (does not pause).
pub struct SuggestActionsTool;

impl SuggestActionsTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SuggestActionsTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SuggestActionsTool {
    fn name(&self) -> &str {
        "suggest_actions"
    }

    fn description(&self) -> &str {
        "Propose follow-up actions after finishing the current task. Does NOT pause execution — \
         use ask_user when you need an answer before continuing. Each action becomes a button; \
         clicking it starts a NEW turn with your `prompt` as the instruction."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "actions": {
                    "type": "array",
                    "description": "Suggested follow-up actions, at most 3. Each becomes a clickable button.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "label": {
                                "type": "string",
                                "description": "Button text shown to the user, at most 12 characters"
                            },
                            "prompt": {
                                "type": "string",
                                "description": "The instruction to run when the user clicks this button. Write it as a complete, self-contained instruction — the user will not retype anything."
                            }
                        },
                        "required": ["label", "prompt"]
                    }
                }
            },
            "required": ["actions"]
        })
    }

    async fn execute(&self, input: Value) -> Result<ToolOutput> {
        // no-op：真正处理在 session.rs 拦截；这里只给万一走到默认 execute 的路径一个可读结果。
        let n = input["actions"].as_array().map(|a| a.len()).unwrap_or(0);
        Ok(ToolOutput {
            content: format!("Suggested {n} follow-up actions"),
            is_error: false,
        })
    }
}
