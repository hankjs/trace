//! QR 登录状态机：admin HTTP 轮询驱动，confirmed 后落库账号并启动 monitor。

use crate::weixin::api::{IlinkClient, QR_API_BASE};
use crate::weixin::monitor;
use crate::AppState;
use anyhow::{anyhow, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// 登录会话 5 分钟过期
const LOGIN_TTL: Duration = Duration::from_secs(300);

pub type LoginStates = RwLock<HashMap<String, LoginEntry>>;

pub struct LoginEntry {
    pub qrcode: String,
    pub qrcode_url: String,
    pub status: LoginStatus,
    pub created_at: Instant,
    /// 防止重复轮询在 confirmed 响应期间并发创建多个账号。
    pub poll_in_flight: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LoginStatus {
    Waiting,
    Scanned,
    Confirmed {
        account_id: String,
        ilink_bot_id: String,
    },
    Expired,
    Error {
        message: String,
    },
}

/// 发起一次登录：取二维码，登记 login_id。
pub async fn start(state: &Arc<AppState>) -> Result<(String, String)> {
    let client = IlinkClient::new();
    let (qrcode, qrcode_url) = client.fetch_qrcode().await?;
    let login_id = uuid::Uuid::new_v4().to_string();
    let mut logins = state.weixin_logins.write().await;
    purge_expired(&mut logins);
    logins.insert(
        login_id.clone(),
        LoginEntry {
            qrcode,
            qrcode_url: qrcode_url.clone(),
            status: LoginStatus::Waiting,
            created_at: Instant::now(),
            poll_in_flight: false,
        },
    );
    Ok((login_id, qrcode_url))
}

/// admin 轮询：代为调用一次 get_qrcode_status 并推进状态。
/// 返回当前状态视图（克隆），confirmed 时落库并启动 monitor。
pub async fn poll(state: &Arc<AppState>, login_id: &str) -> Result<LoginStatus> {
    // 先检查存在性与过期
    {
        let mut logins = state.weixin_logins.write().await;
        purge_expired(&mut logins);
        let entry = logins
            .get_mut(login_id)
            .ok_or_else(|| anyhow!("login_id 不存在或已过期"))?;
        if entry.created_at.elapsed() > LOGIN_TTL
            && !matches!(entry.status, LoginStatus::Confirmed { .. })
        {
            entry.status = LoginStatus::Expired;
        }
        match &entry.status {
            LoginStatus::Waiting | LoginStatus::Scanned => {}
            other => return Ok(other.clone()),
        }
        if entry.poll_in_flight {
            return Ok(entry.status.clone());
        }
        entry.poll_in_flight = true;
    }

    let qrcode = {
        let logins = state.weixin_logins.read().await;
        logins
            .get(login_id)
            .map(|e| e.qrcode.clone())
            .unwrap_or_default()
    };

    let client = IlinkClient::new();
    let resp = match client.poll_qr_status(QR_API_BASE, &qrcode).await {
        Ok(r) => r,
        Err(e) => {
            // 网络错误不算终态，保持 waiting 让前端继续轮询
            tracing::warn!(login_id, "poll_qr_status failed: {e:#}");
            let mut logins = state.weixin_logins.write().await;
            if let Some(entry) = logins.get_mut(login_id) {
                entry.poll_in_flight = false;
                return Ok(entry.status.clone());
            }
            return Ok(LoginStatus::Error {
                message: e.to_string(),
            });
        }
    };

    match resp.status.as_str() {
        "wait" => finish_poll(state, login_id, LoginStatus::Waiting).await,
        "scaned" => finish_poll(state, login_id, LoginStatus::Scanned).await,
        "expired" => finish_poll(state, login_id, LoginStatus::Expired).await,
        "confirmed" => {
            let (bot_token, ilink_bot_id, baseurl) =
                match (resp.bot_token, resp.ilink_bot_id, resp.baseurl) {
                    (Some(t), Some(b), Some(u)) => (t, b, u),
                    _ => {
                        let status = LoginStatus::Error {
                            message: "登录确认但响应缺少 bot_token/ilink_bot_id/baseurl"
                                .to_string(),
                        };
                        return finish_poll(state, login_id, status).await;
                    }
                };
            let account_id = match state
                .db
                .create_weixin_account(
                    &ilink_bot_id,
                    &bot_token,
                    &baseurl,
                    resp.ilink_user_id.as_deref(),
                )
                .await
            {
                Ok(id) => id,
                Err(e) => {
                    if let Some(entry) = state.weixin_logins.write().await.get_mut(login_id) {
                        entry.poll_in_flight = false;
                    }
                    return Err(e);
                }
            };
            match state.db.get_weixin_account(&account_id).await {
                Ok(Some(account)) => monitor::spawn_monitor(state.clone(), account),
                Ok(None) => {
                    if let Some(entry) = state.weixin_logins.write().await.get_mut(login_id) {
                        entry.poll_in_flight = false;
                    }
                    return Err(anyhow!("微信账号已创建但读取失败: {account_id}"));
                }
                Err(e) => {
                    if let Some(entry) = state.weixin_logins.write().await.get_mut(login_id) {
                        entry.poll_in_flight = false;
                    }
                    return Err(e);
                }
            }
            let status = LoginStatus::Confirmed {
                account_id,
                ilink_bot_id,
            };
            finish_poll(state, login_id, status).await
        }
        other => {
            let status = LoginStatus::Error {
                message: format!("未知登录状态: {other}"),
            };
            finish_poll(state, login_id, status).await
        }
    }
}

async fn finish_poll(
    state: &Arc<AppState>,
    login_id: &str,
    status: LoginStatus,
) -> Result<LoginStatus> {
    let mut logins = state.weixin_logins.write().await;
    let entry = logins
        .get_mut(login_id)
        .ok_or_else(|| anyhow!("login_id 不存在或已过期"))?;
    entry.poll_in_flight = false;
    entry.status = status.clone();
    Ok(status)
}

fn purge_expired(logins: &mut HashMap<String, LoginEntry>) {
    // 终态（confirmed/expired/error）多保留一个 TTL，便于 admin 读到最终状态
    logins.retain(|_, e| e.created_at.elapsed() <= LOGIN_TTL * 2);
}
