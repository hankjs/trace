//! ICE（STUN/TURN）服务器配置与限时凭据签发。
//!
//! coturn `use-auth-secret` REST 模式：
//! username = "<过期 unix 时间戳>:<subject>"
//! credential = base64(HMAC-SHA1(secret, username))
//!
//! 不配 turn.secret 时只回公网 STUN（LAN/tailnet 场景够用）。

use crate::config::TurnConfig;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha1::Sha1;

type HmacSha1 = Hmac<Sha1>;

const FALLBACK_STUN: &str = "stun:stun.l.google.com:19302";

/// 返回 RTCPeerConnection 可用的 iceServers 配置。
/// subject 用于区分凭据归属（admin user_id），纯审计，不影响 coturn 校验。
pub fn ice_servers(cfg: &TurnConfig, subject: &str) -> Value {
    let urls: Vec<String> = if cfg.urls.is_empty() {
        vec![FALLBACK_STUN.to_string()]
    } else {
        cfg.urls.clone()
    };

    if cfg.secret.trim().is_empty() {
        return json!({
            "iceServers": [{ "urls": urls }],
            "ttl": 0,
        });
    }

    let ttl = if cfg.ttl_seconds == 0 {
        86400
    } else {
        cfg.ttl_seconds
    };
    let expiry = chrono::Utc::now().timestamp() + ttl as i64;
    let username = format!("{expiry}:{subject}");
    let mut mac = HmacSha1::new_from_slice(cfg.secret.as_bytes())
        .expect("HMAC-SHA1 key length is valid");
    mac.update(username.as_bytes());
    let credential = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());

    json!({
        "iceServers": [{
            "urls": urls,
            "username": username,
            "credential": credential,
        }],
        "ttl": ttl,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_secret_returns_stun_only() {
        let cfg = TurnConfig::default();
        let v = ice_servers(&cfg, "admin-1");
        assert_eq!(v["ttl"], 0);
        assert!(v["iceServers"][0]["urls"][0].as_str().unwrap().starts_with("stun:"));
        assert!(v["iceServers"][0].get("username").is_none());
    }

    #[test]
    fn with_secret_issues_time_limited_credential() {
        let cfg = TurnConfig {
            urls: vec!["turn:example.com:3478".into()],
            secret: "s3cret".into(),
            ttl_seconds: 3600,
        };
        let v = ice_servers(&cfg, "u1");
        assert_eq!(v["ttl"], 3600);
        let user = v["iceServers"][0]["username"].as_str().unwrap();
        assert!(user.ends_with(":u1"));
        assert!(!v["iceServers"][0]["credential"].as_str().unwrap().is_empty());
    }
}
