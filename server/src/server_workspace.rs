//! server-only native 会话的工作区判定与准入。
//!
//! 原先住在 `deployment.rs`，自助部署移除后这两个判定仍被 native 链路需要：
//! - `chat.rs` 用 `is_repository_workspace_metadata` 决定 monorepo 会话的额外禁写路径
//! - `feishu/router.rs` 建 server-only native 会话前用 `ensure_server_agent_admin` 卡权限

use crate::AppState;
use anyhow::{anyhow, bail, Result};
use std::sync::Arc;

/// 会话工作区是否为 Trace monorepo 本体。
///
/// `workspace_kind` 缺失时回落看 `server_agent`——早期会话没写 `workspace_kind`，
/// 那时的 server 会话一律是 monorepo worktree。
pub fn is_repository_workspace_metadata(metadata: Option<&str>) -> bool {
    let Some(value) = metadata.and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
    else {
        return false;
    };
    match value["workspace_kind"].as_str() {
        Some("repository") => true,
        Some(_) => false,
        None => value["server_agent"].as_bool().unwrap_or(false),
    }
}

/// server-only 工作区会话仅限 Trace 管理员创建。
pub async fn ensure_server_agent_admin(state: &Arc<AppState>, user_id: &str) -> Result<()> {
    let user = state
        .db
        .get_user_by_id(user_id)
        .await?
        .ok_or_else(|| anyhow!("Trace 用户不存在"))?;
    if !user.can_login_admin {
        bail!("只有 Trace 管理员可以使用 server-only 代码工作区");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::is_repository_workspace_metadata;

    #[test]
    fn repository_workspace_detection() {
        assert!(is_repository_workspace_metadata(Some(
            r#"{"server_agent":true,"workspace_kind":"repository"}"#
        )));
        // workspace_kind 缺失时回落 server_agent
        assert!(is_repository_workspace_metadata(Some(
            r#"{"server_agent":true}"#
        )));
        assert!(!is_repository_workspace_metadata(Some(
            r#"{"server_agent":true,"workspace_kind":"general"}"#
        )));
        assert!(!is_repository_workspace_metadata(Some(
            r#"{"server_agent":true,"agent_kind":"conversation","workspace_kind":"none"}"#
        )));
        assert!(!is_repository_workspace_metadata(None));
        assert!(!is_repository_workspace_metadata(Some("not json")));
    }
}
