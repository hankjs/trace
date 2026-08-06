//! 通用 API key 认证：外部系统（桥接服务等）不用用户名密码登录即可调 client API。
//!
//! 口径：
//! - key 格式 `trk_` + 32 字节随机的 urlsafe base64（对齐 handy 的 hnk_ 惯例）；
//!   明文只在创建时返回一次，库只存 sha256 哈希（hank-db `api_keys` 表）。
//! - `Authorization: Bearer trk_...` 在 auth_middleware 走 API key 路径，
//!   合成与 JWT 等价的 Claims（client scope，can_admin 恒 false——admin 路由天然拒绝）；
//!   其余 Bearer 照旧走 JWT，两条路径互不影响。
//! - 管理路径：admin REST（本文件 handler）+ 运维 provision 子命令（直连 DB）。

use crate::auth::Claims;
use crate::response::{self as R};
use crate::AppState;
use anyhow::{bail, Context, Result};
use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use hank_db::{ApiKey, Database, User};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// key 明文前缀：auth_middleware 靠它分流 API key / JWT 路径。
pub const KEY_PREFIX: &str = "trk_";

/// 生成 key 明文：`trk_` + 32 字节随机的 urlsafe base64（43 字符）。
/// 随机源用两个 UUID v4（各 122 bit 熵），不为这一个用途新引 rand 依赖。
pub fn generate_key() -> String {
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    format!("{KEY_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes))
}

/// 入库/查库的 key 指纹：sha256 hex（64 字符）。
pub fn hash_key(plaintext: &str) -> String {
    let digest = Sha256::digest(plaintext.as_bytes());
    format!("{digest:x}")
}

/// 查库结果 → 可用 key。key 不存在与吊销是不同的错误文案，但都不泄露更多信息。
fn ensure_usable(key: Option<ApiKey>) -> Result<ApiKey, String> {
    match key {
        None => Err("invalid api key".to_string()),
        Some(k) if k.revoked => Err("api key revoked".to_string()),
        Some(k) => Ok(k),
    }
}

/// API key 路径的身份回显信息：不进 Claims（那是 JWT 的线格式，不动），
/// 由 auth_middleware 作为独立 extension 传给 handler（如 whoami）。
#[derive(Debug, Clone)]
pub struct ApiKeyIdentity {
    pub key_id: String,
    pub key_name: String,
}

/// 合成与 JWT 等价的 Claims。**can_admin 恒 false**：API key 永远拿不到
/// admin 权限，admin_required 中间件天然拒绝。exp 非 JWT 路径无意义，置 0。
pub fn claims_for_api_key(user: &User) -> Claims {
    Claims {
        sub: user.id.clone(),
        username: user.username.clone(),
        can_admin: false,
        can_client: true,
        exp: 0,
    }
}

/// API key 认证：sha256 查表 → 未吊销 → 归属用户存在 → 合成 claims。
/// 返回 claims 与 key 身份信息（供 whoami 等端点回显）。
/// last_used_at 挪到后台 task 更新，不阻塞请求路径。
pub async fn authenticate_api_key(
    state: &Arc<AppState>,
    token: &str,
) -> Result<(Claims, ApiKeyIdentity), String> {
    let key = state
        .db
        .get_api_key_by_hash(&hash_key(token))
        .await
        .map_err(|e| {
            tracing::warn!("api key 查询失败: {e:#}");
            "api key 查询失败".to_string()
        })?;
    let key = ensure_usable(key)?;
    let user = state
        .db
        .get_user_by_id(&key.user_id)
        .await
        .map_err(|e| {
            tracing::warn!(api_key_id = %key.id, "api key 归属用户查询失败: {e:#}");
            "api key 归属用户查询失败".to_string()
        })?
        .ok_or_else(|| "api key 归属用户不存在".to_string())?;

    let identity = ApiKeyIdentity {
        key_id: key.id.clone(),
        key_name: key.name.clone(),
    };
    let db = state.db.clone();
    let key_id = key.id.clone();
    tokio::spawn(async move {
        if let Err(e) = db.touch_api_key_last_used(&key_id).await {
            tracing::warn!(api_key_id = %key_id, "更新 api key last_used_at 失败: {e:#}");
        }
    });

    Ok((claims_for_api_key(&user), identity))
}

// ─── admin REST 管理端点 ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateApiKeyBody {
    /// user_id 与 username 二选一（user_id 优先）
    pub user_id: Option<String>,
    pub username: Option<String>,
    pub name: String,
}

/// POST /api/admin/api-keys  {user_id 或 username, name} → {id, key}
/// 明文只在本次响应返回一次，库只存哈希。
pub async fn create_api_key(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateApiKeyBody>,
) -> impl IntoResponse {
    let name = body.name.trim();
    if name.is_empty() {
        return R::bad_request("name 不能为空");
    }
    let user = match resolve_user(&state.db, body.user_id.as_deref(), body.username.as_deref())
        .await
    {
        Ok(Some(user)) => user,
        Ok(None) => return R::bad_request("用户不存在（需要有效的 user_id 或 username）"),
        Err(e) => return R::internal_error(e),
    };
    let plaintext = generate_key();
    match state
        .db
        .create_api_key(&user.id, name, &hash_key(&plaintext))
        .await
    {
        Ok(row) => R::created(serde_json::json!({
            "id": row.id,
            "key": plaintext,
            "name": row.name,
            "user_id": row.user_id,
        })),
        Err(e) => R::internal_error(e),
    }
}

async fn resolve_user(
    db: &Database,
    user_id: Option<&str>,
    username: Option<&str>,
) -> Result<Option<User>> {
    if let Some(id) = user_id.filter(|s| !s.trim().is_empty()) {
        return db.get_user_by_id(id).await;
    }
    if let Some(name) = username.filter(|s| !s.trim().is_empty()) {
        return db.get_user_by_username(name).await;
    }
    Ok(None)
}

/// GET /api/admin/api-keys —— 列表不含哈希/明文（ApiKey.key_hash 是 skip_serializing）。
pub async fn list_api_keys(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.db.list_api_keys().await {
        Ok(rows) => R::ok(rows),
        Err(e) => R::internal_error(e),
    }
}

/// POST /api/admin/api-keys/{id}/revoke —— 幂等：重复吊销仍返回成功。
pub async fn revoke_api_key(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.db.revoke_api_key(&id).await {
        Ok(true) => R::ok(serde_json::json!({ "id": id, "revoked": true })),
        Ok(false) => R::not_found("api key 不存在"),
        Err(e) => R::internal_error(e),
    }
}

// ─── 运维 provision 子命令（直连 DB，不启动 HTTP 服务）────────────────────
//
// admin REST 需要 admin 密码登录，服务器运维侧需要一条不依赖 UI 的路径。
// 形态选「server 二进制子命令」而非独立 bin：trace 没有 cli/ crate（hank-cli
// 已下线），单 bin 参数分支复用 Config::load + Database::new，零新增构建目标。
//
// 服务器调用方式（cwd 需能读到 config.toml，如 /opt/hank/current）：
//   ./hank-server create-api-key --username <名> --name <key名>   # 明文只打印一次
//   ./hank-server list-api-keys
//   ./hank-server revoke-api-key --id <id>

pub fn is_provision_command(cmd: &str) -> bool {
    matches!(cmd, "create-api-key" | "list-api-keys" | "revoke-api-key")
}

/// `--flag value` 形式的极简参数解析（不为三个子命令引 clap）。
fn flag_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|w| w[0] == name)
        .map(|w| w[1].as_str())
}

pub async fn run_provision(db: &Database, cmd: &str, args: &[String]) -> Result<()> {
    match cmd {
        "create-api-key" => {
            let name = flag_value(args, "--name").context("缺少 --name <key名>")?;
            let user = resolve_user(
                db,
                flag_value(args, "--user-id"),
                flag_value(args, "--username"),
            )
            .await?
            .context("用户不存在（需要 --username 或 --user-id）")?;
            let plaintext = generate_key();
            let row = db
                .create_api_key(&user.id, name, &hash_key(&plaintext))
                .await?;
            // 明文只打印这一次，库只存哈希
            println!("{plaintext}");
            eprintln!(
                "api key 已创建: id={} user={} name={}",
                row.id, user.username, row.name
            );
        }
        "list-api-keys" => {
            for k in db.list_api_keys().await? {
                println!(
                    "{}\tuser_id={}\tname={}\trevoked={}\tcreated_at={}\tlast_used_at={}",
                    k.id,
                    k.user_id,
                    k.name,
                    k.revoked,
                    k.created_at,
                    k.last_used_at
                        .map(|t| t.to_string())
                        .unwrap_or_else(|| "-".to_string())
                );
            }
        }
        "revoke-api-key" => {
            let id = flag_value(args, "--id").context("缺少 --id <key id>")?;
            if !db.revoke_api_key(id).await? {
                bail!("api key 不存在: {id}");
            }
            eprintln!("已吊销: {id}");
        }
        _ => bail!("未知 provision 子命令: {cmd}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api_key(revoked: bool) -> ApiKey {
        ApiKey {
            id: "k1".to_string(),
            user_id: "u1".to_string(),
            name: "bridge".to_string(),
            key_hash: "deadbeef".to_string(),
            revoked,
            created_at: chrono::Utc::now(),
            last_used_at: None,
        }
    }

    fn user() -> User {
        User {
            id: "u1".to_string(),
            username: "bridge-bot".to_string(),
            password_hash: "x".to_string(),
            can_login_admin: true, // 归属用户自己是 admin 也不能抬高 key 的权限
            can_login_client: true,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn hash_key_is_sha256_hex() {
        // sha256("test") 的公开测试向量
        assert_eq!(
            hash_key("test"),
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
        );
        assert_eq!(hash_key("trk_x").len(), 64);
        assert_ne!(hash_key("trk_a"), hash_key("trk_b"));
    }

    #[test]
    fn generate_key_format() {
        let key = generate_key();
        assert!(key.starts_with(KEY_PREFIX), "{key}");
        // trk_ + 32 字节 urlsafe base64（无 padding 43 字符）
        assert_eq!(key.len(), 4 + 43, "{key}");
        assert!(
            key[4..]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "{key}"
        );
        assert_ne!(generate_key(), generate_key());
    }

    #[test]
    fn ensure_usable_rejects_missing_and_revoked() {
        assert_eq!(ensure_usable(None).unwrap_err(), "invalid api key");
        let err = ensure_usable(Some(api_key(true))).unwrap_err();
        assert!(err.contains("revoked"), "{err}");
        let usable = ensure_usable(Some(api_key(false))).unwrap();
        assert_eq!(usable.id, "k1");
    }

    #[test]
    fn claims_for_api_key_is_client_scope_only() {
        let claims = claims_for_api_key(&user());
        assert_eq!(claims.sub, "u1");
        assert_eq!(claims.username, "bridge-bot");
        assert!(!claims.can_admin, "API key 永远拿不到 admin 权限");
        assert!(claims.can_client);
    }

    #[test]
    fn error_envelope_is_401_with_code() {
        let resp = R::unauthorized("api key revoked");
        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn flag_value_parses_pairs() {
        let args = vec![
            "--username".to_string(),
            "bob".to_string(),
            "--name".to_string(),
            "bridge".to_string(),
        ];
        assert_eq!(flag_value(&args, "--username"), Some("bob"));
        assert_eq!(flag_value(&args, "--name"), Some("bridge"));
        assert_eq!(flag_value(&args, "--id"), None);
    }

    #[test]
    fn provision_command_recognition() {
        assert!(is_provision_command("create-api-key"));
        assert!(is_provision_command("list-api-keys"));
        assert!(is_provision_command("revoke-api-key"));
        assert!(!is_provision_command("create_api_key"));
        assert!(!is_provision_command("--help"));
    }
}
