use crate::context::summary::truncate_tool_result_default;
use crate::runtime::RunState;
use crate::types::{FileChange, FileChangeKind};
use crate::AgentEvent;
use code_tools::{
    PermissionDecision, PermissionGuard, Tool, ToolOutput, ToolRisk, DEFAULT_TOOL_TIMEOUT,
};
use hank_provider::ContentBlock;
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// 权限门控结果
pub(crate) enum ToolGate {
    /// 允许执行
    Proceed,
    /// 被拒绝，附带原因（写入 tool_result，loop 继续）
    Denied(String),
}

pub(crate) struct ToolCallContext<'a> {
    pub(crate) id: &'a str,
    pub(crate) name: &'a str,
    pub(crate) input: &'a Value,
    pub(crate) run_id: &'a str,
    pub(crate) turn_id: &'a str,
    pub(crate) call_id: Option<&'a str>,
}

#[derive(Clone)]
pub(crate) struct ToolRuntime {
    permission: Arc<PermissionGuard>,
    tools: Vec<Arc<dyn Tool>>,
    work_dir: String,
}

impl ToolRuntime {
    pub(crate) fn new(
        permission: Arc<PermissionGuard>,
        tools: Vec<Arc<dyn Tool>>,
        work_dir: impl Into<String>,
    ) -> Self {
        Self {
            permission,
            tools,
            work_dir: work_dir.into(),
        }
    }

    pub(crate) fn resolve_path(&self, path: &str) -> String {
        resolve_path_for(path, &self.work_dir)
    }

    /// 查询工具声明的风险等级
    pub(crate) fn tool_risk_for(tools: &[Arc<dyn Tool>], name: &str) -> ToolRisk {
        tools
            .iter()
            .find(|t| t.name() == name)
            .map(|t| t.risk_level())
            .unwrap_or(ToolRisk::Safe)
    }

    /// 根据工具类型与执行前状态推断文件变更（FR-TOOL-6）
    pub(crate) fn detect_file_change_for(
        name: &str,
        input: &Value,
        pre_exists: Option<bool>,
    ) -> Option<FileChange> {
        let path = input["path"].as_str()?.to_string();
        match name {
            "write_file" => {
                let kind = if pre_exists == Some(true) {
                    FileChangeKind::Update
                } else {
                    FileChangeKind::Add
                };
                Some(FileChange { path, kind })
            }
            "str_replace" => Some(FileChange {
                path,
                kind: FileChangeKind::Update,
            }),
            "delete_file" => Some(FileChange {
                path,
                kind: FileChangeKind::Delete,
            }),
            _ => None,
        }
    }

    /// 工具执行前的权限门控（FR-PERM-2/5/6）。
    /// - Allow → Proceed
    /// - Deny → 发 permission.denied，记录 denial，返回 Denied
    /// - NeedApproval → 非交互场景优雅降级为 Denied，发 permission.requested + permission.denied
    pub(crate) async fn gate_tool_with(
        permission: &PermissionGuard,
        tools: &[Arc<dyn Tool>],
        work_dir: &str,
        name: &str,
        input: &Value,
        tool_use_id: &str,
        run_id: &str,
        turn_id: &str,
        event_tx: &mpsc::Sender<AgentEvent>,
        run_state: &mut RunState,
    ) -> ToolGate {
        let risk = Self::tool_risk_for(tools, name);
        let decision = permission.check(name, input, risk, work_dir);
        match decision {
            PermissionDecision::Allow => ToolGate::Proceed,
            PermissionDecision::Deny(reason) => {
                run_state
                    .permission_denials
                    .push(format!("{name}: {reason}"));
                let _ = event_tx
                    .send(AgentEvent::PermissionDenied {
                        run_id: run_id.to_string(),
                        turn_id: turn_id.to_string(),
                        tool: name.to_string(),
                        tool_use_id: tool_use_id.to_string(),
                        reason: reason.clone(),
                    })
                    .await;
                ToolGate::Denied(reason)
            }
            PermissionDecision::NeedApproval(reason) => {
                let _ = event_tx
                    .send(AgentEvent::PermissionRequested {
                        run_id: run_id.to_string(),
                        turn_id: turn_id.to_string(),
                        tool: name.to_string(),
                        tool_use_id: tool_use_id.to_string(),
                        risk: format!("{:?}", risk),
                        reason: reason.clone(),
                    })
                    .await;
                let denial = format!("requires approval: {reason}");
                run_state
                    .permission_denials
                    .push(format!("{name}: {denial}"));
                let _ = event_tx
                    .send(AgentEvent::PermissionDenied {
                        run_id: run_id.to_string(),
                        turn_id: turn_id.to_string(),
                        tool: name.to_string(),
                        tool_use_id: tool_use_id.to_string(),
                        reason: denial.clone(),
                    })
                    .await;
                ToolGate::Denied(denial)
            }
        }
    }

    pub(crate) async fn gate_tool(
        &self,
        call: &ToolCallContext<'_>,
        event_tx: &mpsc::Sender<AgentEvent>,
        run_state: &mut RunState,
    ) -> ToolGate {
        Self::gate_tool_with(
            &self.permission,
            &self.tools,
            &self.work_dir,
            call.name,
            call.input,
            call.id,
            call.run_id,
            call.turn_id,
            event_tx,
            run_state,
        )
        .await
    }

    pub(crate) fn timeout_for(&self, name: &str) -> std::time::Duration {
        for tool in &self.tools {
            if tool.name() == name {
                return tool.timeout();
            }
        }
        DEFAULT_TOOL_TIMEOUT
    }

    pub(crate) async fn execute_tool(
        &self,
        name: &str,
        input: Value,
        event_tx: &mpsc::Sender<AgentEvent>,
        tool_use_id: &str,
    ) -> ToolOutput {
        for tool in &self.tools {
            if tool.name() == name {
                if tool.supports_streaming() {
                    let (stream_tx, mut stream_rx) = mpsc::channel::<String>(64);
                    let event_tx_clone = event_tx.clone();
                    let id_clone = tool_use_id.to_string();

                    let forward_handle = tokio::spawn(async move {
                        while let Some(chunk) = stream_rx.recv().await {
                            let _ = event_tx_clone
                                .send(AgentEvent::ToolOutputDelta {
                                    id: id_clone.clone(),
                                    chunk,
                                })
                                .await;
                        }
                    });

                    let result = match tool.execute_streaming(input, stream_tx).await {
                        Ok(output) => output,
                        Err(e) => ToolOutput {
                            content: format!("Tool execution error: {e}"),
                            is_error: true,
                        },
                    };

                    let _ = forward_handle.await;
                    return result;
                } else {
                    return match tool.execute(input).await {
                        Ok(output) => output,
                        Err(e) => ToolOutput {
                            content: format!("Tool execution error: {e}"),
                            is_error: true,
                        },
                    };
                }
            }
        }
        ToolOutput {
            content: format!("Unknown tool: {name}"),
            is_error: true,
        }
    }

    pub(crate) async fn execute_tool_call(
        &self,
        call: ToolCallContext<'_>,
        event_tx: &mpsc::Sender<AgentEvent>,
        run_state: &mut RunState,
    ) -> ContentBlock {
        match self.gate_tool(&call, event_tx, run_state).await {
            ToolGate::Proceed => {}
            ToolGate::Denied(reason) => {
                let content = format!(
                    "Permission denied: {reason}. This action was not executed. If needed, the user can perform it manually."
                );
                let _ = event_tx
                    .send(AgentEvent::ToolResult {
                        id: call.id.to_string(),
                        name: Some(call.name.to_string()),
                        content: content.clone(),
                        is_error: true,
                        run_id: Some(call.run_id.to_string()),
                        turn_id: Some(call.turn_id.to_string()),
                        call_id: call.call_id.map(str::to_string),
                        duration_ms: Some(0),
                    })
                    .await;
                return ContentBlock::ToolResult {
                    tool_use_id: call.id.to_string(),
                    content,
                    is_error: true,
                };
            }
        }

        let pre_exists = if call.name == "write_file" || call.name == "str_replace" {
            call.input["path"]
                .as_str()
                .map(|p| std::path::Path::new(&self.resolve_path(p)).exists())
        } else {
            None
        };

        let input_str = serde_json::to_string(call.input).unwrap_or_default();
        let timeout = self.timeout_for(call.name);
        debug!("Executing tool: name={}, id={}", call.name, call.id);
        let _ = event_tx
            .send(AgentEvent::ToolStart {
                id: call.id.to_string(),
                name: call.name.to_string(),
                input: input_str,
                run_id: Some(call.run_id.to_string()),
                turn_id: Some(call.turn_id.to_string()),
                call_id: call.call_id.map(str::to_string),
                risk: Some(format!("{:?}", Self::tool_risk_for(&self.tools, call.name))),
                timeout_ms: Some(timeout.as_millis() as u64),
            })
            .await;

        let tool_start = Instant::now();
        let output = match tokio::time::timeout(
            timeout,
            self.execute_tool(call.name, call.input.clone(), event_tx, call.id),
        )
        .await
        {
            Ok(tool_output) => tool_output,
            Err(_) => {
                warn!("Tool {} timed out after {:?}", call.name, timeout);
                ToolOutput {
                    content: format!("Tool execution timed out after {}s", timeout.as_secs()),
                    is_error: true,
                }
            }
        };

        let tool_duration_ms = tool_start.elapsed().as_millis() as u64;
        debug!("Tool result: id={}, is_error={}", call.id, output.is_error);
        let _ = event_tx
            .send(AgentEvent::ToolResult {
                id: call.id.to_string(),
                name: Some(call.name.to_string()),
                content: output.content.clone(),
                is_error: output.is_error,
                run_id: Some(call.run_id.to_string()),
                turn_id: Some(call.turn_id.to_string()),
                call_id: call.call_id.map(str::to_string),
                duration_ms: Some(tool_duration_ms),
            })
            .await;
        let _ = event_tx
            .send(AgentEvent::ToolMetrics {
                tool_name: call.name.to_string(),
                duration_ms: tool_duration_ms,
                is_error: output.is_error,
            })
            .await;

        if !output.is_error {
            if let Some(change) = Self::detect_file_change_for(call.name, call.input, pre_exists) {
                run_state.file_changes.push(change.clone());
                let _ = event_tx
                    .send(AgentEvent::FileChanged {
                        run_id: call.run_id.to_string(),
                        turn_id: call.turn_id.to_string(),
                        changes: vec![change],
                    })
                    .await;
            }
        }

        let content = truncate_tool_result_default(&output.content);
        let content = if output.is_error {
            classify_tool_error(&content, call.name)
        } else {
            content
        };

        ContentBlock::ToolResult {
            tool_use_id: call.id.to_string(),
            content,
            is_error: output.is_error,
        }
    }
}

pub(crate) fn resolve_path_for(path: &str, work_dir: &str) -> String {
    if path.starts_with('/') || work_dir.is_empty() {
        path.to_string()
    } else {
        format!("{}/{}", work_dir.trim_end_matches('/'), path)
    }
}

/// FR-ROBUST-4/5: 工具失败后错误分类，附加语义提示帮助模型选择恢复策略。
pub(crate) fn classify_tool_error(content: &str, tool_name: &str) -> String {
    let lower = content.to_lowercase();
    let category = if lower.contains("command not found")
        || (lower.contains("no such file or directory") && tool_name == "shell")
    {
        "[error_type: command_not_found] The command is not installed. Try an alternative command or check if it needs to be installed first."
    } else if lower.contains("permission denied")
        || lower.contains("access denied")
        || lower.contains("operation not permitted")
    {
        "[error_type: permission_denied] Insufficient permissions. This action requires elevated privileges or is outside the allowed workspace."
    } else if lower.contains("network")
        || lower.contains("dns")
        || lower.contains("connection refused")
        || lower.contains("could not resolve")
    {
        "[error_type: network_failure] Network or DNS failure. The resource may be unreachable; try a local fallback if available."
    } else if lower.contains("not found")
        || lower.contains("does not exist")
        || lower.contains("no such file")
    {
        "[error_type: not_found] File or resource not found. Verify the path or create the missing resource first."
    } else if lower.contains("timed out") || lower.contains("timeout") {
        "[error_type: timeout] Operation timed out. Consider splitting the task or using a faster alternative."
    } else if lower.contains("test")
        && (lower.contains("failed") || lower.contains("error") || lower.contains("assert"))
    {
        "[error_type: test_failure] Tests failed. Read the failure output carefully and make targeted fixes."
    } else {
        "[error_type: tool_error]"
    };
    format!("{category}\n{content}")
}
