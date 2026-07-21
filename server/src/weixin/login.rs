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
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LoginStatus {
    Waiting,
    Scanned,
    Confirmed { account_id: String, ilink_bot_id: String },
    Expired,
    Error { message: String },
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
    }

    let qrcode = {
        let logins = state.weixin_logins.read().await;
        logins.get(login_id).map(|e| e.qrcode.clone()).unwrap_or_default()
    };

    let client = IlinkClient::new();
    let resp = match client.poll_qr_status(QR_API_BASE, &qrcode).await {
        Ok(r) => r,
        Err(e) => {
            // 网络错误不算终态，保持 waiting 让前端继续轮询
            tracing::warn!(login_id, "poll_qr_status failed: {e:#}");
            let logins = state.weixin_logins.read().await;
            return Ok(logins
                .get(login_id)
                .map(|e| e.status.clone())
                .unwrap_or(LoginStatus::Error {
                    message: e.to_string(),
                }));
        }
    };

    match resp.status.as_str() {
        "wait" => Ok(LoginStatus::Waiting),
        "scaned" => {
            let mut logins = state.weixin_logins.write().await;
            if let Some(e) = logins.get_mut(login_id) {
                e.status = LoginStatus::Scanned;
            }
            Ok(LoginStatus::Scanned)
        }
        "expired" => {
            let mut logins = state.weixin_logins.write().await;
            if let Some(e) = logins.get_mut(login_id) {
                e.status = LoginStatus::Expired;
            }
            Ok(LoginStatus::Expired)
        }
        "confirmed" => {
            let (bot_token, ilink_bot_id, baseurl) = match (resp.bot_token, resp.ilink_bot_id, resp.baseurl) {
                (Some(t), Some(b), Some(u)) => (t, b, u),
                _ => {
                    let status = LoginStatus::Error {
                        message: "登录确认但响应缺少 bot_token/ilink_bot_id/baseurl".to_string(),
                    };
                    let mut logins = state.weixin_logins.write().await;
                    if let Some(e) = logins.get_mut(login_id) {
                        e.status = status.clone();
                    }
                    return Ok(status);
                }
            };
            let account_id = state
                .db
                .create_weixin_account(&ilink_bot_id, &bot_token, &baseurl, resp.ilink_user_id.as_deref())
                .await?;
            if let Some(account) = state.db.get_weixin_account(&account_id).await? {
                monitor::spawn_monitor(state.clone(), account);
            }
            let status = LoginStatus::Confirmed {
                account_id,
                ilink_bot_id,
            };
            let mut logins = state.weixin_logins.write().await;
            if let Some(e) = logins.get_mut(login_id) {
                e.status = status.clone();
            }
            Ok(status)
        }
        other => {
            let status = LoginStatus::Error {
                message: format!("未知登录状态: {other}"),
            };
            let mut logins = state.weixin_logins.write().await;
            if let Some(e) = logins.get_mut(login_id) {
                e.status = status.clone();
            }
            Ok(status)
        }
    }
}

fn purge_expired(logins: &mut HashMap<String, LoginEntry>) {
    // 终态（confirmed/expired/error）多保留一个 TTL，便于 admin 读到最终状态
    logins.retain(|_, e| e.created_at.elapsed() <= LOGIN_TTL * 2);
}
