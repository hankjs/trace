//! 常驻 getupdates 长轮询任务：每个 enabled 账号一个 tokio task。

use crate::weixin::api::{IlinkClient, IlinkMessage};
use crate::weixin::router;
use crate::AppState;
use hank_db::WeixinAccount;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const BACKOFF_INIT: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// server 启动时调用：为每个 enabled 账号起一个长轮询循环。
pub fn start_monitors(state: Arc<AppState>) {
    tokio::spawn(async move {
        match state.db.list_weixin_accounts().await {
            Ok(accounts) => {
                for account in accounts.into_iter().filter(|a| a.enabled) {
                    spawn_monitor(state.clone(), account);
                }
            }
            Err(e) => tracing::error!("weixin: list accounts failed: {e:#}"),
        }
    });
}

/// 启动（或重启）某账号的 monitor。已在跑的会先停掉。
pub fn spawn_monitor(state: Arc<AppState>, account: WeixinAccount) {
    if !state.config.server.weixin_monitor {
        tracing::debug!(account_id = %account.id, "weixin monitor disabled by config, skip");
        return;
    }
    let account_id = account.id.clone();
    tokio::spawn(async move {
        stop_monitor(&state, &account_id).await;
        let token = Arc::new(CancellationToken::new());
        {
            let mut monitors = state.weixin_monitors.write().await;
            monitors.insert(account_id.clone(), token.clone());
        }
        tracing::info!(account_id, ilink_bot_id = %account.ilink_bot_id, "weixin monitor started");
        tokio::spawn(monitor_loop(state, account, token));
    });
}

/// 停止某账号的 monitor（若在跑）。
pub async fn stop_monitor(state: &Arc<AppState>, account_id: &str) {
    let token = {
        let mut monitors = state.weixin_monitors.write().await;
        monitors.remove(account_id)
    };
    if let Some(token) = token {
        token.cancel();
        tracing::info!(account_id, "weixin monitor stopped");
    }
}

async fn monitor_loop(state: Arc<AppState>, account: WeixinAccount, token: Arc<CancellationToken>) {
    let client = IlinkClient::new();
    let account_id = account.id.clone();
    // 从 DB 读 cursor 续传
    let mut buf = account.get_updates_buf.clone();
    let mut backoff = BACKOFF_INIT;

    loop {
        if token.is_cancelled() {
            break;
        }
        let result = tokio::select! {
            _ = token.cancelled() => break,
            r = client.get_updates(&account, buf.as_deref()) => r,
        };
        match result {
            Ok(resp) => {
                if resp.token_expired() {
                    tracing::warn!(account_id, "weixin bot token expired, disabling account");
                    let _ = state.db.set_weixin_account_enabled(&account_id, false).await;
                    break;
                }
                backoff = BACKOFF_INIT;
                // cursor 更新后落库
                if let Some(new_buf) = resp.get_updates_buf {
                    if !new_buf.is_empty() && buf.as_deref() != Some(new_buf.as_str()) {
                        buf = Some(new_buf.clone());
                        if let Err(e) = state.db.update_weixin_cursor(&account_id, Some(&new_buf)).await {
                            tracing::warn!(account_id, "weixin: save cursor failed: {e:#}");
                        }
                    }
                }
                for msg in resp.msgs {
                    if msg.message_type == Some(1) {
                        // spawn 处理，不阻塞轮询
                        tokio::spawn(dispatch(state.clone(), account.clone(), msg));
                    }
                }
            }
            Err(e) => {
                tracing::warn!(account_id, "weixin getupdates error: {e:#}, retry in {backoff:?}");
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
    }

    // 清理注册表（仅当表里还是自己这个 token，避免误删重启后的新 monitor）
    let mut monitors = state.weixin_monitors.write().await;
    if monitors.get(&account_id).is_some_and(|t| Arc::ptr_eq(t, &token)) {
        monitors.remove(&account_id);
    }
    tracing::info!(account_id, "weixin monitor exited");
}

async fn dispatch(state: Arc<AppState>, account: WeixinAccount, msg: IlinkMessage) {
    if let Err(e) = router::handle_message(state, account, msg).await {
        tracing::warn!("weixin: handle message failed: {e:#}");
    }
}
