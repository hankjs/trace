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
    /// 峰值上下文 input tokens（单次 LLM 调用上报的最大值，非累计用量）。
    /// 口径说明（P3-#17）：provider 上报的 input_tokens 是该次请求的整个上下文大小，
    /// 累计无意义（会重复计算历史消息），故取 max() 记峰值；
    /// RunCompleted.input_tokens 事件字段沿用此峰值语义。
    pub(crate) peak_input_tokens: u32,
    pub(crate) output_tokens: u32,
    /// 终止原因备注（如 reached max iterations），并入最终 summary（【AF 08】退出带原因）
    pub(crate) termination_note: Option<String>,
}
