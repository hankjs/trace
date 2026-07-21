//! 远程代理工具：与被代理的本地工具同名同 schema，execute 通过
//! remote_exec 通道下发到绑定的桌面 client，在其本地 work_dir 执行。
//!
//! 元数据（name/description/input_schema/is_write/risk_level/timeout）
//! 直接委托给内部持有的本地工具实例，保证与本地执行完全一致，
//! 模型与权限门控无感知；仅 execute 改为远程分发。

use crate::remote_exec::{self, NETWORK_MARGIN};
use crate::AppState;
use anyhow::Result;
use async_trait::async_trait;
use code_tools::{
    git::GitTool, list_directory::ListDirectoryTool, read_file::ReadFileTool, search::SearchTool,
    shell::ShellTool, str_replace::StrReplaceTool, write_file::WriteFileTool, Tool, ToolOutput,
    ToolRisk,
};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

pub struct RemoteTool {
    /// 本地工具实例，仅用于复用元数据（execute 不会被调用）
    inner: Arc<dyn Tool>,
    state: Arc<AppState>,
    user_id: String,
    client_id: String,
}

impl RemoteTool {
    fn wrap(
        inner: Arc<dyn Tool>,
        state: &Arc<AppState>,
        user_id: &str,
        client_id: &str,
    ) -> Arc<dyn Tool> {
        Arc::new(Self {
            inner,
            state: state.clone(),
            user_id: user_id.to_string(),
            client_id: client_id.to_string(),
        })
    }
}

#[async_trait]
impl Tool for RemoteTool {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn input_schema(&self) -> Value {
        self.inner.input_schema()
    }

    /// 被代理工具的 timeout + 网络余量（排队、传输、结果回传）
    fn timeout(&self) -> Duration {
        self.inner.timeout() + NETWORK_MARGIN
    }

    fn is_write(&self) -> bool {
        self.inner.is_write()
    }

    fn risk_level(&self) -> ToolRisk {
        self.inner.risk_level()
    }

    async fn execute(&self, input: Value) -> Result<ToolOutput> {
        if !remote_exec::is_client_online(&self.state, &self.user_id, &self.client_id).await {
            return Ok(ToolOutput {
                content: "Error: 桌面 client 不在线，无法执行本地操作。请提示用户打开 Trace 客户端后重试。"
                    .to_string(),
                is_error: true,
            });
        }
        match remote_exec::dispatch_tool_call(
            &self.state,
            &self.user_id,
            &self.client_id,
            self.inner.name(),
            input,
            self.timeout(),
        )
        .await
        {
            Ok(result) => Ok(ToolOutput {
                content: result.content,
                is_error: result.is_error,
            }),
            Err(e) => Ok(ToolOutput {
                content: format!("Error: 远程执行失败：{e:#}"),
                is_error: true,
            }),
        }
    }
}

/// 组装远程会话的 fs/shell 类工具集（test_runner 不提供，agent 可用 shell 跑测试）。
/// work_dir 为 client 侧路径，仅用于构造本地工具实例（元数据不依赖它）。
pub fn remote_tool_set(
    state: Arc<AppState>,
    user_id: &str,
    client_id: &str,
    work_dir: Option<String>,
) -> Vec<Arc<dyn Tool>> {
    let locals: Vec<Arc<dyn Tool>> = vec![
        Arc::new(ShellTool::new(work_dir.clone())),
        Arc::new(ReadFileTool::new(work_dir.clone())),
        Arc::new(WriteFileTool::new(work_dir.clone())),
        Arc::new(StrReplaceTool::new(work_dir.clone())),
        Arc::new(ListDirectoryTool::new(work_dir.clone())),
        Arc::new(SearchTool::new(work_dir.clone())),
        Arc::new(GitTool::new(work_dir)),
    ];
    locals
        .into_iter()
        .map(|inner| RemoteTool::wrap(inner, &state, user_id, client_id))
        .collect()
}
