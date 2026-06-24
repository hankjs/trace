pub mod events;
pub mod tool_runtime;

use crate::types::FileChange;

pub(crate) use events::{build_run_summary_from, emit_run_terminal, now_ts};
pub(crate) use tool_runtime::{classify_tool_error, ToolCallContext, ToolGate, ToolRuntime};

/// 一次 run 内累积的执行状态（FR-LOOP-7, FR-PERM-6, FR-EVT-2）
#[derive(Default)]
pub(crate) struct RunState {
    pub(crate) run_id: String,
    pub(crate) permission_denials: Vec<String>,
    pub(crate) verification_issues: Vec<String>,
    pub(crate) file_changes: Vec<FileChange>,
    pub(crate) input_tokens: u32,
    pub(crate) output_tokens: u32,
}
