use crate::runtime::RunState;
use crate::types::RunStatus;
use crate::AgentEvent;
use anyhow::Result;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// 当前 UTC 时间戳（RFC3339）
pub(crate) fn now_ts() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// 构造标准化最终汇报：改动文件 + 权限拒绝（FR-LOOP-4 验收 / 第8节）
pub(crate) fn build_run_summary_from(run_state: &RunState) -> String {
    let mut parts: Vec<String> = Vec::new();
    if run_state.file_changes.is_empty() {
        parts.push("No file changes.".to_string());
    } else {
        let files: Vec<String> = run_state
            .file_changes
            .iter()
            .map(|c| format!("{:?} {}", c.kind, c.path))
            .collect();
        parts.push(format!("Changed files: {}", files.join(", ")));
    }
    if !run_state.permission_denials.is_empty() {
        parts.push(format!(
            "Permission denials: {}",
            run_state.permission_denials.join("; ")
        ));
    }
    if !run_state.verification_issues.is_empty() {
        parts.push(format!(
            "Verification issues: {}",
            run_state.verification_issues.join("; ")
        ));
    }
    if let Some(ref note) = run_state.termination_note {
        parts.push(format!("Stopped: {note}"));
    }
    parts.join(" | ")
}

/// 发出 run 终态事件（completed/failed/cancelled）。
/// paused=true 表示因 ask_user 暂停，不发 RunCompleted。
pub(crate) async fn emit_run_terminal(
    run_id: &str,
    run_state: &RunState,
    result: &Result<()>,
    paused: bool,
    cancel: &CancellationToken,
    event_tx: &mpsc::Sender<AgentEvent>,
) {
    match result {
        Err(e) => {
            let _ = event_tx
                .send(AgentEvent::RunFailed {
                    run_id: run_id.to_string(),
                    timestamp: now_ts(),
                    message: format!("{e:#}"),
                })
                .await;
        }
        Ok(()) if paused => {
            // ask_user 暂停：run 未结束，不发终态事件
        }
        Ok(()) if cancel.is_cancelled() => {
            // FR-SESSION-5: 取消后保留 partial file_changes/permission_denials
            let _ = event_tx
                .send(AgentEvent::RunCancelled {
                    run_id: run_id.to_string(),
                    timestamp: now_ts(),
                    file_changes: run_state.file_changes.clone(),
                    permission_denials: run_state.permission_denials.clone(),
                })
                .await;
        }
        Ok(()) => {
            let summary = build_run_summary_from(run_state);
            let _ = event_tx
                .send(AgentEvent::RunCompleted {
                    run_id: run_id.to_string(),
                    timestamp: now_ts(),
                    status: RunStatus::Success,
                    // 峰值上下文 input tokens（非累计，见 RunState 字段注释）
                    input_tokens: run_state.peak_input_tokens,
                    output_tokens: run_state.output_tokens,
                    summary,
                    permission_denials: run_state.permission_denials.clone(),
                    file_changes: run_state.file_changes.clone(),
                })
                .await;
        }
    }
}
