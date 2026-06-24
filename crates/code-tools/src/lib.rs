pub mod ask_user;
pub mod explore_tools;
pub mod file_checksum;
pub mod generate_tools;
pub mod git;
pub mod list_directory;
pub mod permission;
pub mod read_file;
pub mod search;
pub mod shell;
pub mod spec_tools;
pub mod str_replace;
pub mod test_runner;
pub mod web_fetch;
pub mod write_file;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;
use tokio::sync::mpsc;

/// Default tool execution timeout
pub const DEFAULT_TOOL_TIMEOUT: Duration = Duration::from_secs(30);
/// Timeout for long-running tools (shell, compile, test)
pub const LONG_TOOL_TIMEOUT: Duration = Duration::from_secs(300);

pub use permission::ToolRisk;
pub use permission::{
    PermissionConfig, PermissionDecision, PermissionGuard, PermissionMode, DEFAULT_BLOCKED_COMMANDS,
};

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> Value;
    async fn execute(&self, input: Value) -> Result<ToolOutput>;

    /// Timeout hint for this tool. Override for long-running tools like shell.
    fn timeout(&self) -> Duration {
        DEFAULT_TOOL_TIMEOUT
    }

    /// Whether this tool performs write operations (used for concurrency control).
    fn is_write(&self) -> bool {
        false
    }

    /// Risk level for permission system.
    fn risk_level(&self) -> ToolRisk {
        ToolRisk::Safe
    }

    /// Whether this tool supports streaming output.
    fn supports_streaming(&self) -> bool {
        false
    }

    /// Execute with streaming output. Default falls back to regular execute.
    async fn execute_streaming(
        &self,
        input: Value,
        _stream_tx: mpsc::Sender<String>,
    ) -> Result<ToolOutput> {
        self.execute(input).await
    }
}

#[derive(Debug, Clone)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
}
