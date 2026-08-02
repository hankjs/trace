//! 常驻 WS 长连接管理：每个 enabled 飞书账号一个连接，断线指数退避重连。
//!
//! 账号凭证在 feishu_accounts 表（admin REST 管理），启停跟随账号 enabled；
//! 多实例共库时由 server.feishu_monitor 控制只有一个实例开连接（与微信同理）。

use crate::feishu::ws;
use crate::AppState;
use hank_db::FeishuAccount;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const BACKOFF_INIT: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// server 启动时调用：为每个 enabled 账号起一个长连接。
pub fn start_monitors(state: Arc<AppState>) {
    tokio::spawn(async move {
        match state.db.list_feishu_accounts().await {
            Ok(accounts) => {
                for account in accounts.into_iter().filter(|a| a.enabled) {
                    spawn_monitor(state.clone(), account);
                }
            }
            Err(e) => tracing::error!("feishu: list accounts failed: {e:#}"),
        }
    });
}

/// 启动（或重启）某账号的 monitor。已在跑的会先停掉。
pub fn spawn_monitor(state: Arc<AppState>, account: FeishuAccount) {
    if !state.config.server.feishu_monitor {
        tracing::debug!(account_id = %account.id, "feishu monitor disabled by config, skip");
        return;
    }
    let account_id = account.id.clone();
    tokio::spawn(async move {
        stop_monitor(&state, &account_id).await;
        let token = Arc::new(CancellationToken::new());
        {
            let mut monitors = state.feishu_monitors.write().await;
            monitors.insert(account_id.clone(), token.clone());
        }
        tracing::info!(account_id, app_id = %account.app_id, name = %account.name, "feishu monitor started");
        tokio::spawn(monitor_loop(state, account, token));
    });
}

/// 停止某账号的 monitor（若在跑）。
pub async fn stop_monitor(state: &Arc<AppState>, account_id: &str) {
    let token = {
        let mut monitors = state.feishu_monitors.write().await;
        monitors.remove(account_id)
    };
    if let Some(token) = token {
        token.cancel();
        tracing::info!(account_id, "feishu monitor stopped");
    }
}

async fn monitor_loop(state: Arc<AppState>, account: FeishuAccount, token: Arc<CancellationToken>) {
    let account_id = account.id.clone();
    let mut backoff = BACKOFF_INIT;

    loop {
        if token.is_cancelled() {
            break;
        }
        let result = tokio::select! {
            _ = token.cancelled() => break,
            r = ws::connect_and_run(state.clone(), account.clone(), (*token).clone()) => r,
        };
        match result {
            Ok(()) => break, // 正常关闭（cancel）
            Err(e) => {
                tracing::warn!(
                    account_id,
                    "feishu ws disconnected: {e:#}, retry in {backoff:?}"
                );
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
    }

    // 清理注册表（仅当表里还是自己这个 token，避免误删重启后的新 monitor）
    let mut monitors = state.feishu_monitors.write().await;
    if monitors
        .get(&account_id)
        .is_some_and(|t| Arc::ptr_eq(t, &token))
    {
        monitors.remove(&account_id);
    }
    tracing::info!(account_id, "feishu monitor exited");
}
