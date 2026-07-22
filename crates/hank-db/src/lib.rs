use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{mysql::MySqlPoolOptions, MySqlPool};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

/// Retry a database operation with exponential backoff on connection errors.
macro_rules! db_retry {
    ($op:expr) => {{
        let mut attempts = 0u32;
        const MAX_RETRIES: u32 = 4;
        loop {
            match $op.await {
                Ok(v) => break Ok(v),
                Err(e) if attempts < MAX_RETRIES && is_connection_error(&e) => {
                    attempts += 1;
                    let delay_ms = 200u64 * (1u64 << (attempts - 1)); // 200, 400, 800, 1600ms
                    tracing::warn!("DB connection error (attempt {}/{}), retrying in {}ms: {}", attempts, MAX_RETRIES, delay_ms, e);
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
                Err(e) => break Err(anyhow::Error::from(e)),
            }
        }
    }};
}

fn is_connection_error(e: &sqlx::Error) -> bool {
    match e {
        sqlx::Error::Io(_) => true,
        sqlx::Error::PoolClosed => true,
        sqlx::Error::PoolTimedOut => true,
        sqlx::Error::Protocol(_) => false,
        _ => {
            let msg = e.to_string().to_lowercase();
            msg.contains("broken pipe")
                || msg.contains("connection reset")
                || msg.contains("gone away")
                || msg.contains("lost connection")
        }
    }
}

#[derive(Clone)]
pub struct Database {
    pool: MySqlPool,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Session {
    pub id: String,
    pub user_id: Option<String>,
    pub title: String,
    pub provider: String,
    pub model: String,
    pub work_dir: Option<String>,
    pub local_agent: Option<String>,
    pub local_work_dir: Option<String>,
    pub environment: String,
    pub session_type: String,
    pub change_id: Option<String>,
    pub pending_ask_user: Option<String>,
    pub active_leaf_id: Option<String>,
    pub metadata: Option<String>,
    /// 远程执行会话的桌面 client id，NULL 表示 server 本地执行
    pub exec_client_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct DbMessage {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String, // JSON
    pub parent_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNode {
    pub id: String,
    pub parent_id: Option<String>,
    pub role: String,
    pub preview: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Setting {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AgentMetric {
    pub id: String,
    pub session_id: String,
    pub message_id: Option<String>,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub latency_ms: u64,
    pub model: String,
    pub provider: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ToolExecution {
    pub id: String,
    pub session_id: String,
    pub message_id: Option<String>,
    pub tool_name: String,
    pub duration_ms: u64,
    pub is_error: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct PromptTemplate {
    pub id: String,
    pub name: String,
    pub content: String,
    pub category: String,
    pub version: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: String,
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub can_login_admin: bool,
    pub can_login_client: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ProviderRecord {
    pub id: String,
    pub name: String,
    pub provider_type: String,
    pub api_key: String,
    pub base_url: String,
    pub default_model: String,
    pub models: String, // JSON
    pub priority: i32,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AgentEventRecord {
    pub id: String,
    pub session_id: String,
    pub event_type: String,
    pub payload: String,
    pub seq: u64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct LocalEvent {
    pub id: String,
    pub session_id: String,
    pub event_type: String,
    pub agent_type: String,
    pub payload: String,
    pub source: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Checkpoint {
    pub id: String,
    pub session_id: String,
    pub message_id: String,
    pub git_commit_sha: String,
    pub git_branch: String,
    pub spec_snapshot: Option<String>,
    pub label: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Spec {
    pub id: String,
    pub capability: String,
    pub title: String,
    pub content: String,
    pub metadata: Option<String>,
    pub version: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct SpecVersion {
    pub id: String,
    pub spec_id: String,
    pub version: i32,
    pub content: String,
    pub metadata: Option<String>,
    pub change_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Change {
    pub id: String,
    pub name: String,
    pub status: String,
    pub work_dir: Option<String>,
    pub explore_summary: Option<String>,
    pub requirement_path: Option<String>,
    pub tasks_path: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub archived_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ChangeArtifact {
    pub id: String,
    pub change_id: String,
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    pub artifact_type: String,
    pub capability: Option<String>,
    pub content: String,
    pub metadata: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ChangeTask {
    pub id: String,
    pub change_id: String,
    pub group_name: String,
    pub group_order: i32,
    pub task_order: i32,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub session_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct RequirementDoc {
    pub id: String,
    pub change_id: String,
    pub session_id: Option<String>,
    pub name: String,
    pub content: String,
    pub version: i32,
    pub progress_json: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct RequirementDocVersion {
    pub id: String,
    pub doc_id: String,
    pub version: i32,
    pub content: String,
    pub source: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct WeixinAccount {
    pub id: String,
    pub ilink_bot_id: String,
    #[serde(skip_serializing)]
    pub bot_token: String,
    pub base_url: String,
    pub bot_user_id: Option<String>,
    pub get_updates_buf: Option<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct WeixinBinding {
    pub id: String,
    pub account_id: String,
    pub ilink_user_id: String,
    pub user_id: String,
    pub context_token: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct WeixinBindingWithUsername {
    pub id: String,
    pub account_id: String,
    pub ilink_user_id: String,
    pub user_id: String,
    pub username: String,
    pub context_token: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct WeixinBindCode {
    pub code: String,
    pub user_id: String,
    pub expires_at: i64,
    pub used_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct WeixinChat {
    pub id: String,
    pub binding_id: String,
    pub session_id: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ClientAgent {
    pub id: String,
    pub user_id: String,
    pub hostname: Option<String>,
    pub work_dir: Option<String>,
    pub accept_remote: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsOverview {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub avg_latency_ms: f64,
    pub total_llm_calls: u64,
    pub tool_error_count: u64,
    pub tool_total_count: u64,
}

impl Database {
    pub async fn new(database_url: &str) -> Result<Self> {
        let connect_url = Self::maybe_setup_proxy_tunnel(database_url).await?;

        let pool = MySqlPoolOptions::new()
            .max_connections(10)
            .acquire_timeout(Duration::from_secs(10))
            .idle_timeout(Duration::from_secs(300))
            .max_lifetime(Duration::from_secs(1800))
            .test_before_acquire(true)
            .connect(&connect_url)
            .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS sessions (
                id VARCHAR(36) PRIMARY KEY,
                user_id VARCHAR(36) DEFAULT NULL,
                title VARCHAR(255) NOT NULL DEFAULT '',
                provider VARCHAR(64) NOT NULL DEFAULT 'anthropic',
                model VARCHAR(128) NOT NULL DEFAULT '',
                work_dir TEXT DEFAULT NULL,
                local_agent VARCHAR(128) DEFAULT NULL,
                local_work_dir TEXT DEFAULT NULL,
                environment VARCHAR(16) NOT NULL DEFAULT 'remote',
                session_type VARCHAR(16) NOT NULL DEFAULT 'chat',
                change_id VARCHAR(36) DEFAULT NULL,
                pending_ask_user JSON DEFAULT NULL,
                active_leaf_id VARCHAR(36) DEFAULT NULL,
                metadata TEXT DEFAULT NULL,
                exec_client_id VARCHAR(36) DEFAULT NULL,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                updated_at DATETIME NOT NULL DEFAULT NOW(),
                INDEX idx_sessions_user (user_id),
                INDEX idx_sessions_change (change_id)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS messages (
                id VARCHAR(36) PRIMARY KEY,
                session_id VARCHAR(36) NOT NULL,
                role VARCHAR(16) NOT NULL,
                content MEDIUMTEXT NOT NULL,
                parent_id VARCHAR(36) DEFAULT NULL,
                created_at DATETIME(6) NOT NULL DEFAULT NOW(6),
                seq BIGINT NOT NULL DEFAULT 0,
                FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
                INDEX idx_messages_session_seq (session_id, seq, created_at),
                INDEX idx_messages_parent (parent_id)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS settings (
                `key` VARCHAR(255) PRIMARY KEY,
                value TEXT NOT NULL
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS agent_metrics (
                id VARCHAR(36) PRIMARY KEY,
                session_id VARCHAR(36) NOT NULL,
                message_id VARCHAR(36) DEFAULT NULL,
                input_tokens INT UNSIGNED NOT NULL DEFAULT 0,
                output_tokens INT UNSIGNED NOT NULL DEFAULT 0,
                latency_ms BIGINT UNSIGNED NOT NULL DEFAULT 0,
                model VARCHAR(128) NOT NULL DEFAULT '',
                provider VARCHAR(64) NOT NULL DEFAULT '',
                created_at DATETIME NOT NULL DEFAULT NOW(),
                FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
                INDEX idx_agent_metrics_session (session_id)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS tool_executions (
                id VARCHAR(36) PRIMARY KEY,
                session_id VARCHAR(36) NOT NULL,
                message_id VARCHAR(36) DEFAULT NULL,
                tool_name VARCHAR(128) NOT NULL,
                duration_ms BIGINT UNSIGNED NOT NULL DEFAULT 0,
                is_error BOOLEAN NOT NULL DEFAULT FALSE,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
                INDEX idx_tool_executions_session (session_id)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS agent_events (
                id VARCHAR(36) PRIMARY KEY,
                session_id VARCHAR(36) NOT NULL,
                event_type VARCHAR(32) NOT NULL,
                payload MEDIUMTEXT NOT NULL,
                seq BIGINT UNSIGNED NOT NULL DEFAULT 0,
                created_at DATETIME(6) NOT NULL DEFAULT NOW(6),
                FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
                INDEX idx_agent_events_session_seq (session_id, seq)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS prompt_templates (
                id VARCHAR(36) PRIMARY KEY,
                name VARCHAR(255) NOT NULL,
                content MEDIUMTEXT NOT NULL,
                category VARCHAR(32) NOT NULL DEFAULT 'prompt',
                version INT NOT NULL DEFAULT 1,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                INDEX idx_prompt_templates_name (name),
                INDEX idx_prompt_templates_category (category)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Users table
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS users (
                id VARCHAR(36) PRIMARY KEY,
                username VARCHAR(128) NOT NULL UNIQUE,
                password_hash VARCHAR(255) NOT NULL,
                can_login_admin BOOLEAN NOT NULL DEFAULT FALSE,
                can_login_client BOOLEAN NOT NULL DEFAULT TRUE,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                UNIQUE INDEX idx_users_username (username)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Seed default admin user if no users exist
        let user_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
            .fetch_one(&pool)
            .await?;
        if user_count.0 == 0 {
            // 初始 admin 密码: 优先取环境变量, 否则生成随机密码并打印到日志(仅此一次)
            let initial_password = std::env::var("HANK_ADMIN_INITIAL_PASSWORD")
                .ok()
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| Uuid::new_v4().simple().to_string());
            let hash = bcrypt::hash(&initial_password, bcrypt::DEFAULT_COST).unwrap();
            let id = Uuid::new_v4().to_string();
            let _ = sqlx::query(
                "INSERT INTO users (id, username, password_hash, can_login_admin, can_login_client) VALUES (?, ?, ?, TRUE, TRUE)"
            )
            .bind(&id)
            .bind("admin")
            .bind(&hash)
            .execute(&pool)
            .await;
            tracing::warn!(
                "seeded default admin user, initial password: {initial_password} (change it after first login)"
            );
        }

        // Providers table
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS providers (
                id VARCHAR(36) PRIMARY KEY,
                name VARCHAR(128) NOT NULL UNIQUE,
                provider_type VARCHAR(32) NOT NULL,
                api_key VARCHAR(512) NOT NULL,
                base_url VARCHAR(512) NOT NULL DEFAULT '',
                default_model VARCHAR(128) NOT NULL DEFAULT '',
                models TEXT NOT NULL,
                priority INT NOT NULL DEFAULT 0,
                enabled BOOLEAN NOT NULL DEFAULT TRUE,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                INDEX idx_providers_priority (priority)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Image providers table (separate from chat providers)
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS image_providers (
                id VARCHAR(36) PRIMARY KEY,
                name VARCHAR(128) NOT NULL UNIQUE,
                provider_type VARCHAR(32) NOT NULL,
                api_key VARCHAR(512) NOT NULL,
                base_url VARCHAR(512) NOT NULL DEFAULT '',
                default_model VARCHAR(128) NOT NULL DEFAULT '',
                models TEXT NOT NULL,
                priority INT NOT NULL DEFAULT 0,
                enabled BOOLEAN NOT NULL DEFAULT TRUE,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                INDEX idx_image_providers_priority (priority)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Local events table (client-reported ACP execution records)
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS local_events (
                id VARCHAR(36) PRIMARY KEY,
                session_id VARCHAR(36) NOT NULL,
                event_type VARCHAR(64) NOT NULL,
                agent_type VARCHAR(64) NOT NULL DEFAULT '',
                payload MEDIUMTEXT NOT NULL,
                source VARCHAR(16) NOT NULL DEFAULT 'local',
                created_at DATETIME(6) NOT NULL DEFAULT NOW(6),
                FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
                INDEX idx_local_events_session (session_id, created_at)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Specs table
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS specs (
                id VARCHAR(36) PRIMARY KEY,
                capability VARCHAR(255) NOT NULL UNIQUE,
                title VARCHAR(255) NOT NULL,
                content MEDIUMTEXT NOT NULL,
                metadata JSON DEFAULT NULL,
                version INT NOT NULL DEFAULT 1,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                updated_at DATETIME NOT NULL DEFAULT NOW(),
                INDEX idx_specs_capability (capability)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Spec versions table
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS spec_versions (
                id VARCHAR(36) PRIMARY KEY,
                spec_id VARCHAR(36) NOT NULL,
                version INT NOT NULL,
                content MEDIUMTEXT NOT NULL,
                metadata JSON DEFAULT NULL,
                change_id VARCHAR(36) DEFAULT NULL,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                FOREIGN KEY (spec_id) REFERENCES specs(id) ON DELETE CASCADE,
                INDEX idx_spec_versions_spec (spec_id, version)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Checkpoints table
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS checkpoints (
                id VARCHAR(36) PRIMARY KEY,
                session_id VARCHAR(36) NOT NULL,
                message_id VARCHAR(36) NOT NULL,
                git_commit_sha VARCHAR(40) NOT NULL,
                git_branch VARCHAR(255) NOT NULL,
                spec_snapshot JSON DEFAULT NULL,
                label VARCHAR(255) NOT NULL DEFAULT '',
                created_at DATETIME NOT NULL,
                FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
                INDEX idx_checkpoints_session (session_id, created_at)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Changes table
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS changes (
                id VARCHAR(36) PRIMARY KEY,
                name VARCHAR(255) NOT NULL UNIQUE,
                status VARCHAR(32) NOT NULL DEFAULT 'draft',
                work_dir VARCHAR(512) DEFAULT NULL,
                explore_summary TEXT DEFAULT NULL,
                requirement_path VARCHAR(512) DEFAULT NULL,
                tasks_path VARCHAR(512) DEFAULT NULL,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                updated_at DATETIME NOT NULL DEFAULT NOW(),
                archived_at DATETIME DEFAULT NULL,
                INDEX idx_changes_status (status)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Change artifacts table
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS change_artifacts (
                id VARCHAR(36) PRIMARY KEY,
                change_id VARCHAR(36) NOT NULL,
                type VARCHAR(32) NOT NULL,
                capability VARCHAR(255) DEFAULT NULL,
                content MEDIUMTEXT NOT NULL,
                metadata JSON DEFAULT NULL,
                status VARCHAR(16) NOT NULL DEFAULT 'confirmed',
                created_at DATETIME NOT NULL DEFAULT NOW(),
                updated_at DATETIME NOT NULL DEFAULT NOW(),
                FOREIGN KEY (change_id) REFERENCES changes(id) ON DELETE CASCADE,
                UNIQUE KEY uk_change_type_cap (change_id, type, capability),
                INDEX idx_change_artifacts_change (change_id)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Change tasks table
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS change_tasks (
                id VARCHAR(36) PRIMARY KEY,
                change_id VARCHAR(36) NOT NULL,
                group_name VARCHAR(255) NOT NULL,
                group_order INT NOT NULL DEFAULT 0,
                task_order INT NOT NULL DEFAULT 0,
                title VARCHAR(512) NOT NULL,
                description TEXT DEFAULT NULL,
                status VARCHAR(32) NOT NULL DEFAULT 'pending',
                session_id VARCHAR(36) DEFAULT NULL,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                updated_at DATETIME NOT NULL DEFAULT NOW(),
                FOREIGN KEY (change_id) REFERENCES changes(id) ON DELETE CASCADE,
                INDEX idx_change_tasks_change (change_id, group_order, task_order)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Requirement docs table
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS requirement_docs (
                id VARCHAR(36) PRIMARY KEY,
                change_id VARCHAR(36) NOT NULL,
                session_id VARCHAR(36) DEFAULT NULL,
                name VARCHAR(255) NOT NULL,
                content MEDIUMTEXT NOT NULL,
                version INT NOT NULL DEFAULT 1,
                progress_json TEXT DEFAULT NULL,
                status VARCHAR(32) NOT NULL DEFAULT 'draft',
                created_at DATETIME NOT NULL DEFAULT NOW(),
                updated_at DATETIME NOT NULL DEFAULT NOW(),
                INDEX idx_rd_change (change_id),
                INDEX idx_rd_session (session_id)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Requirement doc versions table
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS requirement_doc_versions (
                id VARCHAR(36) PRIMARY KEY,
                doc_id VARCHAR(36) NOT NULL,
                version INT NOT NULL,
                content MEDIUMTEXT NOT NULL,
                source VARCHAR(64) NOT NULL DEFAULT 'system',
                created_at DATETIME NOT NULL DEFAULT NOW(),
                FOREIGN KEY (doc_id) REFERENCES requirement_docs(id) ON DELETE CASCADE,
                INDEX idx_rdv_doc (doc_id, version)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Weixin accounts table (wechat bot login accounts)
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS weixin_accounts (
                id VARCHAR(36) PRIMARY KEY,
                ilink_bot_id VARCHAR(128) NOT NULL,
                bot_token TEXT NOT NULL,
                base_url VARCHAR(255) NOT NULL,
                bot_user_id VARCHAR(128) DEFAULT NULL,
                get_updates_buf MEDIUMTEXT DEFAULT NULL,
                enabled BOOLEAN NOT NULL DEFAULT TRUE,
                created_at DATETIME NOT NULL DEFAULT NOW()
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Weixin bindings table (wechat user <-> trace user)
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS weixin_bindings (
                id VARCHAR(36) PRIMARY KEY,
                account_id VARCHAR(36) NOT NULL,
                ilink_user_id VARCHAR(128) NOT NULL,
                user_id VARCHAR(36) NOT NULL,
                context_token MEDIUMTEXT DEFAULT NULL,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                FOREIGN KEY (account_id) REFERENCES weixin_accounts(id) ON DELETE CASCADE,
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
                UNIQUE KEY uk_weixin_binding (account_id, ilink_user_id)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Weixin bind codes table (one-time binding codes)
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS weixin_bind_codes (
                code VARCHAR(8) PRIMARY KEY,
                user_id VARCHAR(36) NOT NULL,
                expires_at BIGINT NOT NULL,
                used_at BIGINT DEFAULT NULL,
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Weixin chats table (current session mapped to a binding)
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS weixin_chats (
                id VARCHAR(36) PRIMARY KEY,
                binding_id VARCHAR(36) NOT NULL,
                session_id VARCHAR(36) NOT NULL,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                FOREIGN KEY (binding_id) REFERENCES weixin_bindings(id) ON DELETE CASCADE,
                UNIQUE KEY uk_weixin_chat_binding (binding_id)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Client agents table (desktop client registration for remote tool execution)
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS client_agents (
                id VARCHAR(36) PRIMARY KEY,
                user_id VARCHAR(36) NOT NULL,
                hostname VARCHAR(255) DEFAULT NULL,
                work_dir TEXT DEFAULT NULL,
                accept_remote BOOLEAN NOT NULL DEFAULT FALSE,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                updated_at DATETIME NOT NULL DEFAULT NOW(),
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
                INDEX idx_client_agents_user (user_id)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Migrations for existing databases
        // Add category column to prompt_templates if not exists
        let _ = sqlx::query(
            "ALTER TABLE prompt_templates ADD COLUMN category VARCHAR(32) NOT NULL DEFAULT 'prompt' AFTER content"
        ).execute(&pool).await;
        let _ = sqlx::query(
            "ALTER TABLE prompt_templates ADD INDEX idx_prompt_templates_category (category)"
        ).execute(&pool).await;
        // Add requirement_path and tasks_path to changes if not exists
        let _ = sqlx::query(
            "ALTER TABLE changes ADD COLUMN requirement_path VARCHAR(512) DEFAULT NULL AFTER explore_summary"
        ).execute(&pool).await;
        let _ = sqlx::query(
            "ALTER TABLE changes ADD COLUMN tasks_path VARCHAR(512) DEFAULT NULL AFTER requirement_path"
        ).execute(&pool).await;
        // Add exec_client_id to sessions if not exists (remote tool execution target)
        Self::ensure_column(
            &pool,
            "sessions",
            "exec_client_id",
            "ALTER TABLE sessions ADD COLUMN exec_client_id VARCHAR(36) DEFAULT NULL",
        )
        .await?;

        Ok(Self { pool })
    }

    // Check-then-alter: add a column only when it does not exist yet
    async fn ensure_column(pool: &MySqlPool, table: &str, column: &str, ddl: &str) -> Result<()> {
        let exists: Option<(String,)> = sqlx::query_as(
            "SELECT COLUMN_NAME FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ? AND COLUMN_NAME = ?",
        )
        .bind(table)
        .bind(column)
        .fetch_optional(pool)
        .await?;
        if exists.is_none() {
            sqlx::query(ddl).execute(pool).await?;
        }
        Ok(())
    }

    // Sessions
    pub async fn create_session(&self, provider: &str, model: &str, work_dir: Option<&str>, user_id: Option<&str>, environment: Option<&str>, session_type: Option<&str>, metadata: Option<&str>) -> Result<Session> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let env = environment.unwrap_or("remote");
        let s_type = session_type.unwrap_or("chat");
        db_retry!(
            sqlx::query(
                "INSERT INTO sessions (id, user_id, title, provider, model, work_dir, environment, session_type, metadata, created_at, updated_at) VALUES (?, ?, '', ?, ?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(&id)
            .bind(user_id)
            .bind(provider)
            .bind(model)
            .bind(work_dir)
            .bind(env)
            .bind(s_type)
            .bind(metadata)
            .bind(now)
            .bind(now)
            .execute(&self.pool)
        )?;

        Ok(Session {
            id,
            user_id: user_id.map(|s| s.to_string()),
            title: String::new(),
            provider: provider.to_string(),
            model: model.to_string(),
            work_dir: work_dir.map(|s| s.to_string()),
            local_agent: None,
            local_work_dir: None,
            environment: env.to_string(),
            session_type: s_type.to_string(),
            change_id: None,
            pending_ask_user: None,
            active_leaf_id: None,
            metadata: metadata.map(|s| s.to_string()),
            exec_client_id: None,
            created_at: now,
            updated_at: now,
        })
    }

    pub async fn list_sessions(&self) -> Result<Vec<Session>> {
        let sessions = db_retry!(
            sqlx::query_as::<_, Session>(
                "SELECT id, user_id, title, provider, model, work_dir, local_agent, local_work_dir, environment, session_type, change_id, pending_ask_user, active_leaf_id, metadata, exec_client_id, created_at, updated_at FROM sessions ORDER BY updated_at DESC"
            )
            .fetch_all(&self.pool)
        )?;
        Ok(sessions)
    }

    pub async fn list_sessions_by_user(&self, user_id: &str) -> Result<Vec<Session>> {
        let sessions = db_retry!(
            sqlx::query_as::<_, Session>(
                "SELECT id, user_id, title, provider, model, work_dir, local_agent, local_work_dir, environment, session_type, change_id, pending_ask_user, active_leaf_id, metadata, exec_client_id, created_at, updated_at FROM sessions WHERE user_id = ? ORDER BY updated_at DESC"
            )
            .bind(user_id)
            .fetch_all(&self.pool)
        )?;
        Ok(sessions)
    }

    pub async fn get_session(&self, id: &str) -> Result<Option<Session>> {
        let session = db_retry!(
            sqlx::query_as::<_, Session>(
                "SELECT id, user_id, title, provider, model, work_dir, local_agent, local_work_dir, environment, session_type, change_id, pending_ask_user, active_leaf_id, metadata, exec_client_id, created_at, updated_at FROM sessions WHERE id = ?"
            )
            .bind(id)
            .fetch_optional(&self.pool)
        )?;
        Ok(session)
    }

    pub async fn delete_session(&self, id: &str) -> Result<()> {
        db_retry!(
            sqlx::query("DELETE FROM sessions WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn update_session_title(&self, id: &str, title: &str) -> Result<()> {
        db_retry!(
            sqlx::query("UPDATE sessions SET title = ?, updated_at = NOW() WHERE id = ?")
                .bind(title)
                .bind(id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn update_session_work_dir(&self, id: &str, work_dir: Option<&str>) -> Result<()> {
        db_retry!(
            sqlx::query("UPDATE sessions SET work_dir = ?, updated_at = NOW() WHERE id = ?")
                .bind(work_dir)
                .bind(id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn update_session_metadata(&self, id: &str, metadata: &str) -> Result<()> {
        db_retry!(
            sqlx::query("UPDATE sessions SET metadata = ?, updated_at = NOW() WHERE id = ?")
                .bind(metadata)
                .bind(id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn update_session_local_agent(&self, id: &str, local_agent: Option<&str>, local_work_dir: Option<&str>) -> Result<()> {
        db_retry!(
            sqlx::query("UPDATE sessions SET local_agent = ?, local_work_dir = ?, updated_at = NOW() WHERE id = ?")
                .bind(local_agent)
                .bind(local_work_dir)
                .bind(id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    // Messages
    pub async fn save_message(
        &self,
        session_id: &str,
        role: &str,
        content: &serde_json::Value,
        created_at: DateTime<Utc>,
        parent_id: Option<&str>,
    ) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        let content_str = serde_json::to_string(content)?;
        let seq = created_at.timestamp_micros();
        db_retry!(
            sqlx::query("INSERT INTO messages (id, session_id, role, content, created_at, seq, parent_id) VALUES (?, ?, ?, ?, ?, ?, ?)")
                .bind(&id)
                .bind(session_id)
                .bind(role)
                .bind(&content_str)
                .bind(created_at)
                .bind(seq)
                .bind(parent_id)
                .execute(&self.pool)
        )?;
        Ok(id)
    }

    pub async fn touch_session(&self, id: &str) -> Result<()> {
        db_retry!(
            sqlx::query("UPDATE sessions SET updated_at = NOW() WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn get_messages(&self, session_id: &str) -> Result<Vec<DbMessage>> {
        let messages = db_retry!(
            sqlx::query_as::<_, DbMessage>(
                "SELECT id, session_id, role, content, parent_id, created_at FROM messages WHERE session_id = ? ORDER BY seq ASC, created_at ASC"
            )
            .bind(session_id)
            .fetch_all(&self.pool)
        )?;
        Ok(messages)
    }

    /// Walk from leaf_id up to root via parent_id, return messages in root-first order.
    pub async fn get_branch_messages(&self, session_id: &str, leaf_id: &str) -> Result<Vec<DbMessage>> {
        // Load all messages for the session into a map
        let all = self.get_messages(session_id).await?;
        let map: std::collections::HashMap<&str, &DbMessage> =
            all.iter().map(|m| (m.id.as_str(), m)).collect();

        let mut chain = Vec::new();
        let mut current_id = Some(leaf_id);
        while let Some(cid) = current_id {
            if let Some(msg) = map.get(cid) {
                chain.push((*msg).clone());
                current_id = msg.parent_id.as_deref();
            } else {
                break;
            }
        }
        chain.reverse();
        Ok(chain)
    }

    /// Return a flat list of tree nodes for the outline panel.
    pub async fn get_message_tree(&self, session_id: &str) -> Result<Vec<TreeNode>> {
        #[derive(sqlx::FromRow)]
        struct RawNode {
            id: String,
            parent_id: Option<String>,
            role: String,
            content: String,
            created_at: DateTime<Utc>,
        }

        let rows = db_retry!(
            sqlx::query_as::<_, RawNode>(
                "SELECT id, parent_id, role, content, created_at FROM messages WHERE session_id = ? ORDER BY seq ASC, created_at ASC"
            )
            .bind(session_id)
            .fetch_all(&self.pool)
        )?;

        let nodes = rows
            .into_iter()
            .map(|r| {
                // Extract preview: first 30 chars of text content
                let preview = extract_preview(&r.content, 30);
                TreeNode {
                    id: r.id,
                    parent_id: r.parent_id,
                    role: r.role,
                    preview,
                    created_at: r.created_at,
                }
            })
            .collect();
        Ok(nodes)
    }

    pub async fn update_active_leaf(&self, session_id: &str, leaf_id: &str) -> Result<()> {
        db_retry!(
            sqlx::query("UPDATE sessions SET active_leaf_id = ?, updated_at = NOW() WHERE id = ?")
                .bind(leaf_id)
                .bind(session_id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn truncate_messages(&self, session_id: &str, keep_count: u32) -> Result<u64> {
        // Get IDs of messages to keep (first N by ordering)
        let kept_ids: Vec<(String,)> = sqlx::query_as(
            "SELECT id FROM messages WHERE session_id = ? ORDER BY seq ASC, created_at ASC LIMIT ?"
        )
        .bind(session_id)
        .bind(keep_count)
        .fetch_all(&self.pool)
        .await?;

        if kept_ids.is_empty() {
            // Delete all messages for this session
            let result = sqlx::query("DELETE FROM messages WHERE session_id = ?")
                .bind(session_id)
                .execute(&self.pool)
                .await?;
            return Ok(result.rows_affected());
        }

        let ids: Vec<&str> = kept_ids.iter().map(|r| r.0.as_str()).collect();
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let query = format!(
            "DELETE FROM messages WHERE session_id = ? AND id NOT IN ({})",
            placeholders
        );

        let mut q = sqlx::query(&query).bind(session_id);
        for id in &ids {
            q = q.bind(id);
        }
        let result = q.execute(&self.pool).await?;
        Ok(result.rows_affected())
    }

    // Settings
    pub async fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let row = db_retry!(
            sqlx::query_as::<_, Setting>(
                "SELECT `key`, value FROM settings WHERE `key` = ?"
            )
            .bind(key)
            .fetch_optional(&self.pool)
        )?;
        Ok(row.map(|s| s.value))
    }

    pub async fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        db_retry!(
            sqlx::query(
                "INSERT INTO settings (`key`, value) VALUES (?, ?) ON DUPLICATE KEY UPDATE value = VALUES(value)"
            )
            .bind(key)
            .bind(value)
            .execute(&self.pool)
        )?;
        Ok(())
    }

    // Agent Metrics
    pub async fn save_agent_metric(
        &self,
        session_id: &str,
        message_id: Option<&str>,
        input_tokens: u32,
        output_tokens: u32,
        latency_ms: u64,
        model: &str,
        provider: &str,
    ) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        db_retry!(
            sqlx::query(
                "INSERT INTO agent_metrics (id, session_id, message_id, input_tokens, output_tokens, latency_ms, model, provider) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(&id)
            .bind(session_id)
            .bind(message_id)
            .bind(input_tokens)
            .bind(output_tokens)
            .bind(latency_ms)
            .bind(model)
            .bind(provider)
            .execute(&self.pool)
        )?;
        Ok(id)
    }

    pub async fn get_session_metrics(&self, session_id: &str) -> Result<Vec<AgentMetric>> {
        let rows = db_retry!(
            sqlx::query_as::<_, AgentMetric>(
                "SELECT id, session_id, message_id, input_tokens, output_tokens, latency_ms, model, provider, created_at FROM agent_metrics WHERE session_id = ? ORDER BY created_at ASC"
            )
            .bind(session_id)
            .fetch_all(&self.pool)
        )?;
        Ok(rows)
    }

    // Tool Executions
    pub async fn save_tool_execution(
        &self,
        session_id: &str,
        message_id: Option<&str>,
        tool_name: &str,
        duration_ms: u64,
        is_error: bool,
    ) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        db_retry!(
            sqlx::query(
                "INSERT INTO tool_executions (id, session_id, message_id, tool_name, duration_ms, is_error) VALUES (?, ?, ?, ?, ?, ?)"
            )
            .bind(&id)
            .bind(session_id)
            .bind(message_id)
            .bind(tool_name)
            .bind(duration_ms)
            .bind(is_error)
            .execute(&self.pool)
        )?;
        Ok(id)
    }

    pub async fn get_session_tool_executions(&self, session_id: &str) -> Result<Vec<ToolExecution>> {
        let rows = db_retry!(
            sqlx::query_as::<_, ToolExecution>(
                "SELECT id, session_id, message_id, tool_name, duration_ms, is_error, created_at FROM tool_executions WHERE session_id = ? ORDER BY created_at ASC"
            )
            .bind(session_id)
            .fetch_all(&self.pool)
        )?;
        Ok(rows)
    }

    // Agent Events
    pub async fn save_agent_event(
        &self,
        session_id: &str,
        event_type: &str,
        payload: &str,
        seq: u64,
    ) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        db_retry!(
            sqlx::query(
                "INSERT INTO agent_events (id, session_id, event_type, payload, seq) VALUES (?, ?, ?, ?, ?)"
            )
            .bind(&id)
            .bind(session_id)
            .bind(event_type)
            .bind(payload)
            .bind(seq)
            .execute(&self.pool)
        )?;
        Ok(id)
    }

    pub async fn get_session_events(&self, session_id: &str) -> Result<Vec<AgentEventRecord>> {
        let rows = db_retry!(
            sqlx::query_as::<_, AgentEventRecord>(
                "SELECT id, session_id, event_type, payload, seq, created_at FROM agent_events WHERE session_id = ? ORDER BY seq ASC"
            )
            .bind(session_id)
            .fetch_all(&self.pool)
        )?;
        Ok(rows)
    }

    // Prompt Templates
    pub async fn save_prompt_template(&self, name: &str, content: &str, category: Option<&str>) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        let cat = category.unwrap_or("prompt");
        // Get next version for this name
        let max_version: Option<(i32,)> = sqlx::query_as(
            "SELECT COALESCE(MAX(version), 0) FROM prompt_templates WHERE name = ?"
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        let version = max_version.map(|r| r.0).unwrap_or(0) + 1;

        db_retry!(
            sqlx::query(
                "INSERT INTO prompt_templates (id, name, content, category, version) VALUES (?, ?, ?, ?, ?)"
            )
            .bind(&id)
            .bind(name)
            .bind(content)
            .bind(cat)
            .bind(version)
            .execute(&self.pool)
        )?;
        Ok(id)
    }

    pub async fn list_prompt_templates(&self) -> Result<Vec<PromptTemplate>> {
        let rows = db_retry!(
            sqlx::query_as::<_, PromptTemplate>(
                "SELECT id, name, content, category, version, created_at FROM prompt_templates ORDER BY name ASC, version DESC"
            )
            .fetch_all(&self.pool)
        )?;
        Ok(rows)
    }

    pub async fn get_templates_by_category(&self, category: &str) -> Result<Vec<PromptTemplate>> {
        let rows = db_retry!(
            sqlx::query_as::<_, PromptTemplate>(
                "SELECT id, name, content, category, version, created_at FROM prompt_templates WHERE category = ? ORDER BY name ASC, version DESC"
            )
            .bind(category)
            .fetch_all(&self.pool)
        )?;
        Ok(rows)
    }

    pub async fn get_prompt_template(&self, id: &str) -> Result<Option<PromptTemplate>> {
        let row = db_retry!(
            sqlx::query_as::<_, PromptTemplate>(
                "SELECT id, name, content, category, version, created_at FROM prompt_templates WHERE id = ?"
            )
            .bind(id)
            .fetch_optional(&self.pool)
        )?;
        Ok(row)
    }

    pub async fn delete_prompt_template(&self, id: &str) -> Result<()> {
        db_retry!(
            sqlx::query("DELETE FROM prompt_templates WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    // Users
    pub async fn get_user_by_username(&self, username: &str) -> Result<Option<User>> {
        let row = db_retry!(
            sqlx::query_as::<_, User>(
                "SELECT id, username, password_hash, can_login_admin, can_login_client, created_at FROM users WHERE username = ?"
            )
            .bind(username)
            .fetch_optional(&self.pool)
        )?;
        Ok(row)
    }

    pub async fn get_user_by_id(&self, id: &str) -> Result<Option<User>> {
        let row = db_retry!(
            sqlx::query_as::<_, User>(
                "SELECT id, username, password_hash, can_login_admin, can_login_client, created_at FROM users WHERE id = ?"
            )
            .bind(id)
            .fetch_optional(&self.pool)
        )?;
        Ok(row)
    }

    pub async fn list_users(&self) -> Result<Vec<User>> {
        let rows = db_retry!(
            sqlx::query_as::<_, User>(
                "SELECT id, username, password_hash, can_login_admin, can_login_client, created_at FROM users ORDER BY created_at ASC"
            )
            .fetch_all(&self.pool)
        )?;
        Ok(rows)
    }

    pub async fn create_user(&self, username: &str, password: &str, can_admin: bool, can_client: bool) -> Result<User> {
        let id = Uuid::new_v4().to_string();
        let hash = bcrypt::hash(password, bcrypt::DEFAULT_COST)
            .map_err(|e| anyhow::anyhow!("bcrypt error: {}", e))?;
        let now = Utc::now();
        db_retry!(
            sqlx::query(
                "INSERT INTO users (id, username, password_hash, can_login_admin, can_login_client, created_at) VALUES (?, ?, ?, ?, ?, ?)"
            )
            .bind(&id)
            .bind(username)
            .bind(&hash)
            .bind(can_admin)
            .bind(can_client)
            .bind(now)
            .execute(&self.pool)
        )?;
        Ok(User { id, username: username.to_string(), password_hash: hash, can_login_admin: can_admin, can_login_client: can_client, created_at: now })
    }

    pub async fn update_user_permissions(&self, id: &str, can_admin: bool, can_client: bool) -> Result<()> {
        db_retry!(
            sqlx::query("UPDATE users SET can_login_admin = ?, can_login_client = ? WHERE id = ?")
                .bind(can_admin)
                .bind(can_client)
                .bind(id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn update_user_password(&self, id: &str, password: &str) -> Result<()> {
        let hash = bcrypt::hash(password, bcrypt::DEFAULT_COST)
            .map_err(|e| anyhow::anyhow!("bcrypt error: {}", e))?;
        db_retry!(
            sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
                .bind(&hash)
                .bind(id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn delete_user(&self, id: &str) -> Result<()> {
        db_retry!(
            sqlx::query("DELETE FROM users WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    // Providers
    pub async fn list_providers_ordered(&self) -> Result<Vec<ProviderRecord>> {
        let rows = db_retry!(
            sqlx::query_as::<_, ProviderRecord>(
                "SELECT id, name, provider_type, api_key, base_url, default_model, models, priority, enabled, created_at FROM providers ORDER BY priority ASC"
            )
            .fetch_all(&self.pool)
        )?;
        Ok(rows)
    }

    pub async fn get_provider_by_name(&self, name: &str) -> Result<Option<ProviderRecord>> {
        let row = db_retry!(
            sqlx::query_as::<_, ProviderRecord>(
                "SELECT id, name, provider_type, api_key, base_url, default_model, models, priority, enabled, created_at FROM providers WHERE name = ?"
            )
            .bind(name)
            .fetch_optional(&self.pool)
        )?;
        Ok(row)
    }

    pub async fn create_provider(
        &self,
        name: &str,
        provider_type: &str,
        api_key: &str,
        base_url: &str,
        default_model: &str,
        models: &str,
        priority: i32,
        enabled: bool,
    ) -> Result<ProviderRecord> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        db_retry!(
            sqlx::query(
                "INSERT INTO providers (id, name, provider_type, api_key, base_url, default_model, models, priority, enabled, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(&id)
            .bind(name)
            .bind(provider_type)
            .bind(api_key)
            .bind(base_url)
            .bind(default_model)
            .bind(models)
            .bind(priority)
            .bind(enabled)
            .bind(now)
            .execute(&self.pool)
        )?;
        Ok(ProviderRecord {
            id, name: name.to_string(), provider_type: provider_type.to_string(),
            api_key: api_key.to_string(), base_url: base_url.to_string(),
            default_model: default_model.to_string(), models: models.to_string(),
            priority, enabled, created_at: now,
        })
    }

    pub async fn update_provider(
        &self,
        id: &str,
        name: &str,
        provider_type: &str,
        api_key: &str,
        base_url: &str,
        default_model: &str,
        models: &str,
        priority: i32,
        enabled: bool,
    ) -> Result<()> {
        db_retry!(
            sqlx::query(
                "UPDATE providers SET name=?, provider_type=?, api_key=?, base_url=?, default_model=?, models=?, priority=?, enabled=? WHERE id=?"
            )
            .bind(name)
            .bind(provider_type)
            .bind(api_key)
            .bind(base_url)
            .bind(default_model)
            .bind(models)
            .bind(priority)
            .bind(enabled)
            .bind(id)
            .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn delete_provider(&self, id: &str) -> Result<()> {
        db_retry!(
            sqlx::query("DELETE FROM providers WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    // Image Providers
    pub async fn list_image_providers_ordered(&self) -> Result<Vec<ProviderRecord>> {
        let rows = db_retry!(
            sqlx::query_as::<_, ProviderRecord>(
                "SELECT id, name, provider_type, api_key, base_url, default_model, models, priority, enabled, created_at FROM image_providers ORDER BY priority ASC"
            )
            .fetch_all(&self.pool)
        )?;
        Ok(rows)
    }

    pub async fn create_image_provider(
        &self, name: &str, provider_type: &str, api_key: &str,
        base_url: &str, default_model: &str, models: &str, priority: i32, enabled: bool,
    ) -> Result<ProviderRecord> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        db_retry!(
            sqlx::query(
                "INSERT INTO image_providers (id, name, provider_type, api_key, base_url, default_model, models, priority, enabled, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(&id).bind(name).bind(provider_type).bind(api_key)
            .bind(base_url).bind(default_model).bind(models).bind(priority).bind(enabled).bind(now)
            .execute(&self.pool)
        )?;
        Ok(ProviderRecord {
            id, name: name.to_string(), provider_type: provider_type.to_string(),
            api_key: api_key.to_string(), base_url: base_url.to_string(),
            default_model: default_model.to_string(), models: models.to_string(),
            priority, enabled, created_at: now,
        })
    }

    pub async fn update_image_provider(
        &self, id: &str, name: &str, provider_type: &str, api_key: &str,
        base_url: &str, default_model: &str, models: &str, priority: i32, enabled: bool,
    ) -> Result<()> {
        db_retry!(
            sqlx::query(
                "UPDATE image_providers SET name=?, provider_type=?, api_key=?, base_url=?, default_model=?, models=?, priority=?, enabled=? WHERE id=?"
            )
            .bind(name).bind(provider_type).bind(api_key).bind(base_url)
            .bind(default_model).bind(models).bind(priority).bind(enabled).bind(id)
            .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn delete_image_provider(&self, id: &str) -> Result<()> {
        db_retry!(
            sqlx::query("DELETE FROM image_providers WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn provider_count(&self) -> Result<i64> {
        let count: (i64,) = db_retry!(
            sqlx::query_as("SELECT COUNT(*) FROM providers")
                .fetch_one(&self.pool)
        )?;
        Ok(count.0)
    }

    // Aggregated metrics for admin overview
    pub async fn get_metrics_overview(&self) -> Result<MetricsOverview> {
        #[derive(sqlx::FromRow)]
        struct AggRow {
            total_input_tokens: Option<i64>,
            total_output_tokens: Option<i64>,
            avg_latency_ms: Option<f64>,
            total_calls: i64,
        }
        let agg: AggRow = sqlx::query_as(
            "SELECT CAST(COALESCE(SUM(input_tokens), 0) AS SIGNED) as total_input_tokens, CAST(COALESCE(SUM(output_tokens), 0) AS SIGNED) as total_output_tokens, CAST(AVG(latency_ms) AS DOUBLE) as avg_latency_ms, COUNT(*) as total_calls FROM agent_metrics"
        )
        .fetch_one(&self.pool)
        .await?;

        #[derive(sqlx::FromRow)]
        struct ErrRow {
            error_count: i64,
            total_count: i64,
        }
        let err: ErrRow = sqlx::query_as(
            "SELECT CAST(COALESCE(SUM(is_error), 0) AS SIGNED) as error_count, COUNT(*) as total_count FROM tool_executions"
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(MetricsOverview {
            total_input_tokens: agg.total_input_tokens.unwrap_or(0) as u64,
            total_output_tokens: agg.total_output_tokens.unwrap_or(0) as u64,
            avg_latency_ms: agg.avg_latency_ms.unwrap_or(0.0),
            total_llm_calls: agg.total_calls as u64,
            tool_error_count: err.error_count as u64,
            tool_total_count: err.total_count as u64,
        })
    }

    /// Detect proxy env vars and set up a local TCP tunnel if needed.
    /// Returns the database URL to use (possibly rewritten to point at localhost tunnel).
    async fn maybe_setup_proxy_tunnel(database_url: &str) -> Result<String> {
        let proxy_url = std::env::var("all_proxy")
            .or_else(|_| std::env::var("ALL_PROXY"))
            .or_else(|_| std::env::var("https_proxy"))
            .or_else(|_| std::env::var("HTTPS_PROXY"))
            .or_else(|_| std::env::var("http_proxy"))
            .or_else(|_| std::env::var("HTTP_PROXY"));

        let proxy_url = match proxy_url {
            Ok(u) if !u.is_empty() => u,
            _ => return Ok(database_url.to_string()),
        };

        // Parse the MySQL URL to extract host and port
        let parsed = url::Url::parse(database_url)?;
        let db_host = parsed.host_str().unwrap_or("127.0.0.1").to_string();
        let db_port = parsed.port().unwrap_or(3306);

        // Parse proxy URL
        let proxy_parsed = url::Url::parse(&proxy_url)?;
        let proxy_host = proxy_parsed.host_str().unwrap_or("127.0.0.1").to_string();
        let proxy_port = proxy_parsed.port().unwrap_or(7890);

        // Bind a local listener on a random port
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let local_port = listener.local_addr()?.port();

        tracing::info!(
            "MySQL proxy tunnel: 127.0.0.1:{} -> {}:{} via {}:{}",
            local_port, db_host, db_port, proxy_host, proxy_port
        );

        // Spawn the tunnel forwarder
        let target_host = db_host.clone();
        let target_port = db_port;
        tokio::spawn(async move {
            loop {
                let (client, _) = match listener.accept().await {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::error!("Tunnel accept error: {}", e);
                        continue;
                    }
                };
                let proxy_h = proxy_host.clone();
                let target_h = target_host.clone();
                tokio::spawn(async move {
                    if let Err(e) =
                        Self::handle_tunnel(client, &proxy_h, proxy_port, &target_h, target_port).await
                    {
                        tracing::error!("Tunnel connection error: {}", e);
                    }
                });
            }
        });

        // Rewrite the database URL to connect through the local tunnel
        let mut new_url = parsed.clone();
        new_url.set_host(Some("127.0.0.1"))?;
        new_url.set_port(Some(local_port)).map_err(|_| anyhow::anyhow!("failed to set port"))?;

        Ok(new_url.to_string())
    }

    /// Establish an HTTP CONNECT tunnel through the proxy and relay data.
    async fn handle_tunnel(
        mut client: TcpStream,
        proxy_host: &str,
        proxy_port: u16,
        target_host: &str,
        target_port: u16,
    ) -> Result<()> {
        // Connect to the proxy
        let mut proxy = TcpStream::connect(format!("{}:{}", proxy_host, proxy_port)).await?;

        // Send HTTP CONNECT request
        let connect_req = format!(
            "CONNECT {}:{} HTTP/1.1\r\nHost: {}:{}\r\n\r\n",
            target_host, target_port, target_host, target_port
        );
        proxy.write_all(connect_req.as_bytes()).await?;

        // Read the proxy response
        let mut buf = [0u8; 1024];
        let n = proxy.read(&mut buf).await?;
        let response = String::from_utf8_lossy(&buf[..n]);

        if !response.contains("200") {
            anyhow::bail!("Proxy CONNECT failed: {}", response.trim());
        }

        // Relay data between client and proxy
        tokio::io::copy_bidirectional(&mut client, &mut proxy).await?;
        Ok(())
    }

    // Local Events
    pub async fn insert_local_events(&self, events: &[LocalEvent]) -> Result<()> {
        for event in events {
            db_retry!(
                sqlx::query(
                    "INSERT INTO local_events (id, session_id, event_type, agent_type, payload, source, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)"
                )
                .bind(&event.id)
                .bind(&event.session_id)
                .bind(&event.event_type)
                .bind(&event.agent_type)
                .bind(&event.payload)
                .bind(&event.source)
                .bind(event.created_at)
                .execute(&self.pool)
            )?;
        }
        Ok(())
    }

    pub async fn get_local_events(&self, session_id: &str) -> Result<Vec<LocalEvent>> {
        let events = db_retry!(
            sqlx::query_as::<_, LocalEvent>(
                "SELECT id, session_id, event_type, agent_type, payload, source, created_at FROM local_events WHERE session_id = ? ORDER BY created_at ASC"
            )
            .bind(session_id)
            .fetch_all(&self.pool)
        )?;
        Ok(events)
    }

    // ─── Specs ───────────────────────────────────────────────────────────

    pub async fn create_spec(&self, capability: &str, title: &str, content: &str, metadata: Option<&str>) -> Result<Spec> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        db_retry!(
            sqlx::query(
                "INSERT INTO specs (id, capability, title, content, metadata, version, created_at, updated_at) VALUES (?, ?, ?, ?, ?, 1, ?, ?)"
            )
            .bind(&id)
            .bind(capability)
            .bind(title)
            .bind(content)
            .bind(metadata)
            .bind(now)
            .bind(now)
            .execute(&self.pool)
        )?;
        Ok(Spec { id, capability: capability.to_string(), title: title.to_string(), content: content.to_string(), metadata: metadata.map(|s| s.to_string()), version: 1, created_at: now, updated_at: now })
    }

    pub async fn list_specs(&self) -> Result<Vec<Spec>> {
        let specs = db_retry!(
            sqlx::query_as::<_, Spec>(
                "SELECT id, capability, title, content, metadata, version, created_at, updated_at FROM specs ORDER BY capability ASC"
            )
            .fetch_all(&self.pool)
        )?;
        Ok(specs)
    }

    pub async fn get_spec(&self, id: &str) -> Result<Option<Spec>> {
        let spec = db_retry!(
            sqlx::query_as::<_, Spec>(
                "SELECT id, capability, title, content, metadata, version, created_at, updated_at FROM specs WHERE id = ?"
            )
            .bind(id)
            .fetch_optional(&self.pool)
        )?;
        Ok(spec)
    }

    pub async fn get_spec_by_capability(&self, capability: &str) -> Result<Option<Spec>> {
        let spec = db_retry!(
            sqlx::query_as::<_, Spec>(
                "SELECT id, capability, title, content, metadata, version, created_at, updated_at FROM specs WHERE capability = ?"
            )
            .bind(capability)
            .fetch_optional(&self.pool)
        )?;
        Ok(spec)
    }

    pub async fn update_spec(&self, id: &str, content: Option<&str>, metadata: Option<&str>, title: Option<&str>) -> Result<()> {
        let now = Utc::now();
        if let Some(c) = content {
            db_retry!(
                sqlx::query("UPDATE specs SET content = ?, updated_at = ?, version = version + 1 WHERE id = ?")
                    .bind(c)
                    .bind(now)
                    .bind(id)
                    .execute(&self.pool)
            )?;
        }
        if let Some(m) = metadata {
            db_retry!(
                sqlx::query("UPDATE specs SET metadata = ?, updated_at = ? WHERE id = ?")
                    .bind(m)
                    .bind(now)
                    .bind(id)
                    .execute(&self.pool)
            )?;
        }
        if let Some(t) = title {
            db_retry!(
                sqlx::query("UPDATE specs SET title = ?, updated_at = ? WHERE id = ?")
                    .bind(t)
                    .bind(now)
                    .bind(id)
                    .execute(&self.pool)
            )?;
        }
        Ok(())
    }

    pub async fn delete_spec(&self, id: &str) -> Result<()> {
        db_retry!(
            sqlx::query("DELETE FROM specs WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    // ─── Spec Versions ───────────────────────────────────────────────────

    pub async fn create_spec_version(&self, spec_id: &str, version: i32, content: &str, metadata: Option<&str>, change_id: Option<&str>) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        db_retry!(
            sqlx::query(
                "INSERT INTO spec_versions (id, spec_id, version, content, metadata, change_id, created_at) VALUES (?, ?, ?, ?, ?, ?, NOW())"
            )
            .bind(&id)
            .bind(spec_id)
            .bind(version)
            .bind(content)
            .bind(metadata)
            .bind(change_id)
            .execute(&self.pool)
        )?;
        Ok(id)
    }

    pub async fn list_spec_versions(&self, spec_id: &str) -> Result<Vec<SpecVersion>> {
        let versions = db_retry!(
            sqlx::query_as::<_, SpecVersion>(
                "SELECT id, spec_id, version, content, metadata, change_id, created_at FROM spec_versions WHERE spec_id = ? ORDER BY version DESC"
            )
            .bind(spec_id)
            .fetch_all(&self.pool)
        )?;
        Ok(versions)
    }

    // ─── Changes ─────────────────────────────────────────────────────────

    pub async fn create_change(&self, name: &str, work_dir: Option<&str>, requirement_path: Option<&str>, tasks_path: Option<&str>) -> Result<Change> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        db_retry!(
            sqlx::query(
                "INSERT INTO changes (id, name, status, work_dir, requirement_path, tasks_path, created_at, updated_at) VALUES (?, ?, 'draft', ?, ?, ?, ?, ?)"
            )
            .bind(&id)
            .bind(name)
            .bind(work_dir)
            .bind(requirement_path)
            .bind(tasks_path)
            .bind(now)
            .bind(now)
            .execute(&self.pool)
        )?;
        Ok(Change { id, name: name.to_string(), status: "draft".to_string(), work_dir: work_dir.map(|s| s.to_string()), explore_summary: None, requirement_path: requirement_path.map(|s| s.to_string()), tasks_path: tasks_path.map(|s| s.to_string()), created_at: now, updated_at: now, archived_at: None })
    }

    pub async fn list_changes(&self, status: Option<&str>) -> Result<Vec<Change>> {
        let changes = if let Some(s) = status {
            db_retry!(
                sqlx::query_as::<_, Change>(
                    "SELECT id, name, status, work_dir, explore_summary, created_at, updated_at, archived_at FROM changes WHERE status = ? ORDER BY updated_at DESC"
                )
                .bind(s)
                .fetch_all(&self.pool)
            )?
        } else {
            db_retry!(
                sqlx::query_as::<_, Change>(
                    "SELECT id, name, status, work_dir, explore_summary, created_at, updated_at, archived_at FROM changes WHERE status != 'archived' ORDER BY updated_at DESC"
                )
                .fetch_all(&self.pool)
            )?
        };
        Ok(changes)
    }

    pub async fn get_change(&self, id: &str) -> Result<Option<Change>> {
        let change = db_retry!(
            sqlx::query_as::<_, Change>(
                "SELECT id, name, status, work_dir, explore_summary, created_at, updated_at, archived_at FROM changes WHERE id = ?"
            )
            .bind(id)
            .fetch_optional(&self.pool)
        )?;
        Ok(change)
    }

    pub async fn update_change(&self, id: &str, name: Option<&str>, status: Option<&str>) -> Result<()> {
        let now = Utc::now();
        if let Some(n) = name {
            db_retry!(
                sqlx::query("UPDATE changes SET name = ?, updated_at = ? WHERE id = ?")
                    .bind(n)
                    .bind(now)
                    .bind(id)
                    .execute(&self.pool)
            )?;
        }
        if let Some(s) = status {
            if s == "archived" {
                db_retry!(
                    sqlx::query("UPDATE changes SET status = ?, updated_at = ?, archived_at = ? WHERE id = ?")
                        .bind(s)
                        .bind(now)
                        .bind(now)
                        .bind(id)
                        .execute(&self.pool)
                )?;
            } else {
                db_retry!(
                    sqlx::query("UPDATE changes SET status = ?, updated_at = ? WHERE id = ?")
                        .bind(s)
                        .bind(now)
                        .bind(id)
                        .execute(&self.pool)
                )?;
            }
        }
        Ok(())
    }

    pub async fn delete_change(&self, id: &str) -> Result<()> {
        db_retry!(
            sqlx::query("DELETE FROM changes WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn list_changes_by_work_dir(&self, work_dir: &str) -> Result<Vec<Change>> {
        let changes = db_retry!(
            sqlx::query_as::<_, Change>(
                "SELECT id, name, status, work_dir, explore_summary, created_at, updated_at, archived_at FROM changes WHERE work_dir = ? AND status != 'archived' ORDER BY updated_at DESC"
            )
            .bind(work_dir)
            .fetch_all(&self.pool)
        )?;
        Ok(changes)
    }

    pub async fn update_change_explore_summary(&self, change_id: &str, summary: &str) -> Result<()> {
        db_retry!(
            sqlx::query("UPDATE changes SET explore_summary = ?, updated_at = NOW() WHERE id = ?")
                .bind(summary)
                .bind(change_id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    // ─── Session: pending_ask_user & change_id ──────────────────────────

    pub async fn get_session_pending_ask_user(&self, session_id: &str) -> Result<Option<String>> {
        let row: Option<(Option<String>,)> = db_retry!(
            sqlx::query_as("SELECT pending_ask_user FROM sessions WHERE id = ?")
                .bind(session_id)
                .fetch_optional(&self.pool)
        )?;
        Ok(row.and_then(|r| r.0))
    }

    pub async fn set_session_pending_ask_user(&self, session_id: &str, json: &str) -> Result<()> {
        db_retry!(
            sqlx::query("UPDATE sessions SET pending_ask_user = ?, updated_at = NOW() WHERE id = ?")
                .bind(json)
                .bind(session_id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn clear_session_pending_ask_user(&self, session_id: &str) -> Result<()> {
        db_retry!(
            sqlx::query("UPDATE sessions SET pending_ask_user = NULL, updated_at = NOW() WHERE id = ?")
                .bind(session_id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn get_session_by_change_id(&self, change_id: &str) -> Result<Option<Session>> {
        let session = db_retry!(
            sqlx::query_as::<_, Session>(
                "SELECT id, user_id, title, provider, model, work_dir, local_agent, local_work_dir, environment, session_type, change_id, pending_ask_user, active_leaf_id, metadata, exec_client_id, created_at, updated_at FROM sessions WHERE change_id = ? ORDER BY updated_at DESC LIMIT 1"
            )
            .bind(change_id)
            .fetch_optional(&self.pool)
        )?;
        Ok(session)
    }

    pub async fn set_session_change_id(&self, session_id: &str, change_id: &str) -> Result<()> {
        db_retry!(
            sqlx::query("UPDATE sessions SET change_id = ?, updated_at = NOW() WHERE id = ?")
                .bind(change_id)
                .bind(session_id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    // ─── Change Artifacts ────────────────────────────────────────────────

    pub async fn create_artifact(&self, change_id: &str, artifact_type: &str, capability: Option<&str>, content: &str, metadata: Option<&str>, status: Option<&str>) -> Result<ChangeArtifact> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let st = status.unwrap_or("confirmed");
        db_retry!(
            sqlx::query(
                "INSERT INTO change_artifacts (id, change_id, `type`, capability, content, metadata, status, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(&id)
            .bind(change_id)
            .bind(artifact_type)
            .bind(capability)
            .bind(content)
            .bind(metadata)
            .bind(st)
            .bind(now)
            .bind(now)
            .execute(&self.pool)
        )?;
        Ok(ChangeArtifact { id, change_id: change_id.to_string(), artifact_type: artifact_type.to_string(), capability: capability.map(|s| s.to_string()), content: content.to_string(), metadata: metadata.map(|s| s.to_string()), status: st.to_string(), created_at: now, updated_at: now })
    }

    pub async fn list_artifacts(&self, change_id: &str) -> Result<Vec<ChangeArtifact>> {
        let artifacts = db_retry!(
            sqlx::query_as::<_, ChangeArtifact>(
                "SELECT id, change_id, `type` as artifact_type, capability, content, metadata, status, created_at, updated_at FROM change_artifacts WHERE change_id = ? ORDER BY created_at ASC"
            )
            .bind(change_id)
            .fetch_all(&self.pool)
        )?;
        Ok(artifacts)
    }

    pub async fn get_artifact(&self, id: &str) -> Result<Option<ChangeArtifact>> {
        let artifact = db_retry!(
            sqlx::query_as::<_, ChangeArtifact>(
                "SELECT id, change_id, `type` as artifact_type, capability, content, metadata, status, created_at, updated_at FROM change_artifacts WHERE id = ?"
            )
            .bind(id)
            .fetch_optional(&self.pool)
        )?;
        Ok(artifact)
    }

    pub async fn update_artifact(&self, id: &str, content: Option<&str>, metadata: Option<&str>, status: Option<&str>) -> Result<()> {
        let now = Utc::now();
        if let Some(c) = content {
            db_retry!(
                sqlx::query("UPDATE change_artifacts SET content = ?, updated_at = ? WHERE id = ?")
                    .bind(c)
                    .bind(now)
                    .bind(id)
                    .execute(&self.pool)
            )?;
        }
        if let Some(m) = metadata {
            db_retry!(
                sqlx::query("UPDATE change_artifacts SET metadata = ?, updated_at = ? WHERE id = ?")
                    .bind(m)
                    .bind(now)
                    .bind(id)
                    .execute(&self.pool)
            )?;
        }
        if let Some(s) = status {
            db_retry!(
                sqlx::query("UPDATE change_artifacts SET status = ?, updated_at = ? WHERE id = ?")
                    .bind(s)
                    .bind(now)
                    .bind(id)
                    .execute(&self.pool)
            )?;
        }
        Ok(())
    }

    pub async fn delete_artifact(&self, id: &str) -> Result<()> {
        db_retry!(
            sqlx::query("DELETE FROM change_artifacts WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn confirm_artifacts(&self, change_id: &str) -> Result<()> {
        db_retry!(
            sqlx::query("UPDATE change_artifacts SET status = 'confirmed', updated_at = NOW() WHERE change_id = ? AND status = 'draft'")
                .bind(change_id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    // ─── Change Tasks ────────────────────────────────────────────────────

    pub async fn batch_create_tasks(&self, change_id: &str, tasks: &[(String, i32, i32, String, Option<String>)]) -> Result<Vec<ChangeTask>> {
        let now = Utc::now();
        let mut created = Vec::new();
        for (group_name, group_order, task_order, title, description) in tasks {
            let id = Uuid::new_v4().to_string();
            db_retry!(
                sqlx::query(
                    "INSERT INTO change_tasks (id, change_id, group_name, group_order, task_order, title, description, status, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?)"
                )
                .bind(&id)
                .bind(change_id)
                .bind(group_name)
                .bind(group_order)
                .bind(task_order)
                .bind(title)
                .bind(description.as_deref())
                .bind(now)
                .bind(now)
                .execute(&self.pool)
            )?;
            created.push(ChangeTask {
                id, change_id: change_id.to_string(), group_name: group_name.clone(),
                group_order: *group_order, task_order: *task_order, title: title.clone(),
                description: description.clone(), status: "pending".to_string(),
                session_id: None, created_at: now, updated_at: now,
            });
        }
        Ok(created)
    }

    pub async fn list_tasks(&self, change_id: &str) -> Result<Vec<ChangeTask>> {
        let tasks = db_retry!(
            sqlx::query_as::<_, ChangeTask>(
                "SELECT id, change_id, group_name, group_order, task_order, title, description, status, session_id, created_at, updated_at FROM change_tasks WHERE change_id = ? ORDER BY group_order ASC, task_order ASC"
            )
            .bind(change_id)
            .fetch_all(&self.pool)
        )?;
        Ok(tasks)
    }

    pub async fn update_task(&self, id: &str, status: Option<&str>, title: Option<&str>, description: Option<&str>, session_id: Option<&str>) -> Result<()> {
        let now = Utc::now();
        if let Some(s) = status {
            db_retry!(
                sqlx::query("UPDATE change_tasks SET status = ?, updated_at = ? WHERE id = ?")
                    .bind(s)
                    .bind(now)
                    .bind(id)
                    .execute(&self.pool)
            )?;
        }
        if let Some(t) = title {
            db_retry!(
                sqlx::query("UPDATE change_tasks SET title = ?, updated_at = ? WHERE id = ?")
                    .bind(t)
                    .bind(now)
                    .bind(id)
                    .execute(&self.pool)
            )?;
        }
        if let Some(d) = description {
            db_retry!(
                sqlx::query("UPDATE change_tasks SET description = ?, updated_at = ? WHERE id = ?")
                    .bind(d)
                    .bind(now)
                    .bind(id)
                    .execute(&self.pool)
            )?;
        }
        if let Some(sid) = session_id {
            db_retry!(
                sqlx::query("UPDATE change_tasks SET session_id = ?, updated_at = ? WHERE id = ?")
                    .bind(sid)
                    .bind(now)
                    .bind(id)
                    .execute(&self.pool)
            )?;
        }
        Ok(())
    }

    pub async fn delete_task(&self, id: &str) -> Result<()> {
        db_retry!(
            sqlx::query("DELETE FROM change_tasks WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn get_change_task_counts(&self, change_id: &str) -> Result<(i64, i64, i64, i64)> {
        let total: (i64,) = db_retry!(
            sqlx::query_as("SELECT COUNT(*) FROM change_tasks WHERE change_id = ?")
                .bind(change_id)
                .fetch_one(&self.pool)
        )?;
        let done: (i64,) = db_retry!(
            sqlx::query_as("SELECT COUNT(*) FROM change_tasks WHERE change_id = ? AND status = 'done'")
                .bind(change_id)
                .fetch_one(&self.pool)
        )?;
        let in_progress: (i64,) = db_retry!(
            sqlx::query_as("SELECT COUNT(*) FROM change_tasks WHERE change_id = ? AND status = 'in_progress'")
                .bind(change_id)
                .fetch_one(&self.pool)
        )?;
        let pending = total.0 - done.0 - in_progress.0;
        Ok((total.0, done.0, in_progress.0, pending))
    }

    // ─── Checkpoints ─────────────────────────────────────────────────────

    pub async fn create_checkpoint(
        &self,
        session_id: &str,
        message_id: &str,
        git_commit_sha: &str,
        git_branch: &str,
        spec_snapshot: Option<&str>,
        label: &str,
    ) -> Result<Checkpoint> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        db_retry!(
            sqlx::query(
                "INSERT INTO checkpoints (id, session_id, message_id, git_commit_sha, git_branch, spec_snapshot, label, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(&id)
            .bind(session_id)
            .bind(message_id)
            .bind(git_commit_sha)
            .bind(git_branch)
            .bind(spec_snapshot)
            .bind(label)
            .bind(now)
            .execute(&self.pool)
        )?;
        Ok(Checkpoint {
            id,
            session_id: session_id.to_string(),
            message_id: message_id.to_string(),
            git_commit_sha: git_commit_sha.to_string(),
            git_branch: git_branch.to_string(),
            spec_snapshot: spec_snapshot.map(|s| s.to_string()),
            label: label.to_string(),
            created_at: now,
        })
    }

    pub async fn list_checkpoints(&self, session_id: &str) -> Result<Vec<Checkpoint>> {
        let checkpoints = db_retry!(
            sqlx::query_as::<_, Checkpoint>(
                "SELECT id, session_id, message_id, git_commit_sha, git_branch, spec_snapshot, label, created_at FROM checkpoints WHERE session_id = ? ORDER BY created_at ASC"
            )
            .bind(session_id)
            .fetch_all(&self.pool)
        )?;
        Ok(checkpoints)
    }

    pub async fn get_checkpoint(&self, id: &str) -> Result<Option<Checkpoint>> {
        let cp = db_retry!(
            sqlx::query_as::<_, Checkpoint>(
                "SELECT id, session_id, message_id, git_commit_sha, git_branch, spec_snapshot, label, created_at FROM checkpoints WHERE id = ?"
            )
            .bind(id)
            .fetch_optional(&self.pool)
        )?;
        Ok(cp)
    }

    pub async fn delete_checkpoints_after(&self, session_id: &str, created_at: DateTime<Utc>) -> Result<()> {
        db_retry!(
            sqlx::query("DELETE FROM checkpoints WHERE session_id = ? AND created_at > ?")
                .bind(session_id)
                .bind(created_at)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    // ─── Requirement Docs ───────────────────────────────────────────────

    pub async fn create_requirement_doc(&self, change_id: &str, session_id: Option<&str>, name: &str, content: &str, progress_json: Option<&str>) -> Result<RequirementDoc> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        db_retry!(
            sqlx::query(
                "INSERT INTO requirement_docs (id, change_id, session_id, name, content, version, progress_json, status, created_at, updated_at) VALUES (?, ?, ?, ?, ?, 1, ?, 'draft', ?, ?)"
            )
            .bind(&id)
            .bind(change_id)
            .bind(session_id)
            .bind(name)
            .bind(content)
            .bind(progress_json)
            .bind(now)
            .bind(now)
            .execute(&self.pool)
        )?;
        // Also save version 1
        let vid = Uuid::new_v4().to_string();
        db_retry!(
            sqlx::query(
                "INSERT INTO requirement_doc_versions (id, doc_id, version, content, source, created_at) VALUES (?, ?, 1, ?, 'system', ?)"
            )
            .bind(&vid)
            .bind(&id)
            .bind(content)
            .bind(now)
            .execute(&self.pool)
        )?;
        Ok(RequirementDoc { id, change_id: change_id.to_string(), session_id: session_id.map(|s| s.to_string()), name: name.to_string(), content: content.to_string(), version: 1, progress_json: progress_json.map(|s| s.to_string()), status: "draft".to_string(), created_at: now, updated_at: now })
    }

    pub async fn update_requirement_doc(&self, id: &str, content: &str, progress_json: Option<&str>, status: Option<&str>, source: &str) -> Result<()> {
        let now = Utc::now();
        // Increment version
        db_retry!(
            sqlx::query(
                "UPDATE requirement_docs SET content = ?, version = version + 1, progress_json = COALESCE(?, progress_json), status = COALESCE(?, status), updated_at = ? WHERE id = ?"
            )
            .bind(content)
            .bind(progress_json)
            .bind(status)
            .bind(now)
            .bind(id)
            .execute(&self.pool)
        )?;
        // Get new version number
        let row: (i32,) = db_retry!(
            sqlx::query_as("SELECT version FROM requirement_docs WHERE id = ?")
                .bind(id)
                .fetch_one(&self.pool)
        )?;
        // Save version snapshot
        let vid = Uuid::new_v4().to_string();
        db_retry!(
            sqlx::query(
                "INSERT INTO requirement_doc_versions (id, doc_id, version, content, source, created_at) VALUES (?, ?, ?, ?, ?, ?)"
            )
            .bind(&vid)
            .bind(id)
            .bind(row.0)
            .bind(content)
            .bind(source)
            .bind(now)
            .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn get_requirement_doc(&self, id: &str) -> Result<Option<RequirementDoc>> {
        let doc = db_retry!(
            sqlx::query_as::<_, RequirementDoc>(
                "SELECT id, change_id, session_id, name, content, version, progress_json, status, created_at, updated_at FROM requirement_docs WHERE id = ?"
            )
            .bind(id)
            .fetch_optional(&self.pool)
        )?;
        Ok(doc)
    }

    pub async fn get_requirement_doc_by_change(&self, change_id: &str) -> Result<Option<RequirementDoc>> {
        let doc = db_retry!(
            sqlx::query_as::<_, RequirementDoc>(
                "SELECT id, change_id, session_id, name, content, version, progress_json, status, created_at, updated_at FROM requirement_docs WHERE change_id = ? ORDER BY created_at DESC LIMIT 1"
            )
            .bind(change_id)
            .fetch_optional(&self.pool)
        )?;
        Ok(doc)
    }

    pub async fn list_requirement_docs(&self, search: Option<&str>, status: Option<&str>, page: u32, page_size: u32) -> Result<(Vec<RequirementDoc>, u64)> {
        let offset = (page.saturating_sub(1)) * page_size;
        let mut where_clauses = Vec::new();
        if search.is_some() { where_clauses.push("(name LIKE CONCAT('%', ?, '%') OR content LIKE CONCAT('%', ?, '%'))"); }
        if status.is_some() { where_clauses.push("status = ?"); }
        let where_sql = if where_clauses.is_empty() { String::new() } else { format!("WHERE {}", where_clauses.join(" AND ")) };

        let count_sql = format!("SELECT COUNT(*) FROM requirement_docs {}", where_sql);
        let list_sql = format!("SELECT id, change_id, session_id, name, content, version, progress_json, status, created_at, updated_at FROM requirement_docs {} ORDER BY updated_at DESC LIMIT ? OFFSET ?", where_sql);

        // Build count query
        let mut count_q = sqlx::query_as::<_, (i64,)>(&count_sql);
        if let Some(s) = search { count_q = count_q.bind(s).bind(s); }
        if let Some(st) = status { count_q = count_q.bind(st); }
        let (total,): (i64,) = count_q.fetch_one(&self.pool).await?;

        // Build list query
        let mut list_q = sqlx::query_as::<_, RequirementDoc>(&list_sql);
        if let Some(s) = search { list_q = list_q.bind(s).bind(s); }
        if let Some(st) = status { list_q = list_q.bind(st); }
        list_q = list_q.bind(page_size).bind(offset);
        let docs = list_q.fetch_all(&self.pool).await?;

        Ok((docs, total as u64))
    }

    pub async fn list_all_tasks(&self, status: Option<&str>, change_id: Option<&str>, page: u32, page_size: u32) -> Result<(Vec<ChangeTask>, u64)> {
        let offset = (page.saturating_sub(1)) * page_size;
        let mut where_clauses = Vec::new();
        if status.is_some() { where_clauses.push("status = ?"); }
        if change_id.is_some() { where_clauses.push("change_id = ?"); }
        let where_sql = if where_clauses.is_empty() { String::new() } else { format!("WHERE {}", where_clauses.join(" AND ")) };

        let count_sql = format!("SELECT COUNT(*) FROM change_tasks {}", where_sql);
        let list_sql = format!("SELECT id, change_id, group_name, group_order, task_order, title, description, status, session_id, created_at, updated_at FROM change_tasks {} ORDER BY created_at DESC LIMIT ? OFFSET ?", where_sql);

        let mut count_q = sqlx::query_as::<_, (i64,)>(&count_sql);
        if let Some(s) = status { count_q = count_q.bind(s); }
        if let Some(c) = change_id { count_q = count_q.bind(c); }
        let (total,): (i64,) = count_q.fetch_one(&self.pool).await?;

        let mut list_q = sqlx::query_as::<_, ChangeTask>(&list_sql);
        if let Some(s) = status { list_q = list_q.bind(s); }
        if let Some(c) = change_id { list_q = list_q.bind(c); }
        list_q = list_q.bind(page_size).bind(offset);
        let tasks = list_q.fetch_all(&self.pool).await?;

        Ok((tasks, total as u64))
    }

    // Weixin accounts
    pub async fn create_weixin_account(&self, ilink_bot_id: &str, bot_token: &str, base_url: &str, bot_user_id: Option<&str>) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        db_retry!(
            sqlx::query(
                "INSERT INTO weixin_accounts (id, ilink_bot_id, bot_token, base_url, bot_user_id, created_at) VALUES (?, ?, ?, ?, ?, ?)"
            )
            .bind(&id)
            .bind(ilink_bot_id)
            .bind(bot_token)
            .bind(base_url)
            .bind(bot_user_id)
            .bind(now)
            .execute(&self.pool)
        )?;
        Ok(id)
    }

    pub async fn list_weixin_accounts(&self) -> Result<Vec<WeixinAccount>> {
        let rows = db_retry!(
            sqlx::query_as::<_, WeixinAccount>(
                "SELECT id, ilink_bot_id, bot_token, base_url, bot_user_id, get_updates_buf, enabled, created_at FROM weixin_accounts ORDER BY created_at ASC"
            )
            .fetch_all(&self.pool)
        )?;
        Ok(rows)
    }

    pub async fn get_weixin_account(&self, id: &str) -> Result<Option<WeixinAccount>> {
        let row = db_retry!(
            sqlx::query_as::<_, WeixinAccount>(
                "SELECT id, ilink_bot_id, bot_token, base_url, bot_user_id, get_updates_buf, enabled, created_at FROM weixin_accounts WHERE id = ?"
            )
            .bind(id)
            .fetch_optional(&self.pool)
        )?;
        Ok(row)
    }

    pub async fn update_weixin_token(&self, id: &str, bot_token: &str, base_url: &str) -> Result<()> {
        db_retry!(
            sqlx::query("UPDATE weixin_accounts SET bot_token = ?, base_url = ? WHERE id = ?")
                .bind(bot_token)
                .bind(base_url)
                .bind(id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn update_weixin_bot_user_id(&self, id: &str, bot_user_id: &str) -> Result<()> {
        db_retry!(
            sqlx::query("UPDATE weixin_accounts SET bot_user_id = ? WHERE id = ?")
                .bind(bot_user_id)
                .bind(id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn update_weixin_cursor(&self, id: &str, buf: Option<&str>) -> Result<()> {
        db_retry!(
            sqlx::query("UPDATE weixin_accounts SET get_updates_buf = ? WHERE id = ?")
                .bind(buf)
                .bind(id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn set_weixin_account_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        db_retry!(
            sqlx::query("UPDATE weixin_accounts SET enabled = ? WHERE id = ?")
                .bind(enabled)
                .bind(id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn delete_weixin_account(&self, id: &str) -> Result<()> {
        db_retry!(
            sqlx::query("DELETE FROM weixin_accounts WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    // Weixin bindings
    pub async fn create_weixin_binding(&self, account_id: &str, ilink_user_id: &str, user_id: &str) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        db_retry!(
            sqlx::query(
                "INSERT INTO weixin_bindings (id, account_id, ilink_user_id, user_id, created_at) VALUES (?, ?, ?, ?, ?)"
            )
            .bind(&id)
            .bind(account_id)
            .bind(ilink_user_id)
            .bind(user_id)
            .bind(now)
            .execute(&self.pool)
        )?;
        Ok(id)
    }

    pub async fn get_weixin_binding(&self, account_id: &str, ilink_user_id: &str) -> Result<Option<WeixinBinding>> {
        let row = db_retry!(
            sqlx::query_as::<_, WeixinBinding>(
                "SELECT id, account_id, ilink_user_id, user_id, context_token, created_at FROM weixin_bindings WHERE account_id = ? AND ilink_user_id = ?"
            )
            .bind(account_id)
            .bind(ilink_user_id)
            .fetch_optional(&self.pool)
        )?;
        Ok(row)
    }

    pub async fn get_weixin_binding_by_user(&self, user_id: &str) -> Result<Option<WeixinBinding>> {
        let row = db_retry!(
            sqlx::query_as::<_, WeixinBinding>(
                "SELECT id, account_id, ilink_user_id, user_id, context_token, created_at FROM weixin_bindings WHERE user_id = ?"
            )
            .bind(user_id)
            .fetch_optional(&self.pool)
        )?;
        Ok(row)
    }

    pub async fn list_weixin_bindings(&self) -> Result<Vec<WeixinBindingWithUsername>> {
        let rows = db_retry!(
            sqlx::query_as::<_, WeixinBindingWithUsername>(
                "SELECT b.id, b.account_id, b.ilink_user_id, b.user_id, u.username, b.context_token, b.created_at FROM weixin_bindings b JOIN users u ON u.id = b.user_id ORDER BY b.created_at ASC"
            )
            .fetch_all(&self.pool)
        )?;
        Ok(rows)
    }

    pub async fn update_weixin_binding_context(&self, id: &str, context_token: &str) -> Result<()> {
        db_retry!(
            sqlx::query("UPDATE weixin_bindings SET context_token = ? WHERE id = ?")
                .bind(context_token)
                .bind(id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn delete_weixin_binding(&self, id: &str) -> Result<()> {
        db_retry!(
            sqlx::query("DELETE FROM weixin_bindings WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    // Weixin bind codes
    pub async fn create_weixin_bind_code(&self, code: &str, user_id: &str, expires_at: i64) -> Result<()> {
        db_retry!(
            sqlx::query("INSERT INTO weixin_bind_codes (code, user_id, expires_at) VALUES (?, ?, ?)")
                .bind(code)
                .bind(user_id)
                .bind(expires_at)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    /// 校验并消费绑定码：未过期且未使用时原子标记 used，返回 user_id。
    pub async fn consume_weixin_bind_code(&self, code: &str) -> Result<Option<String>> {
        let now_ms = Utc::now().timestamp_millis();
        let res = db_retry!(
            sqlx::query("UPDATE weixin_bind_codes SET used_at = ? WHERE code = ? AND used_at IS NULL AND expires_at > ?")
                .bind(now_ms)
                .bind(code)
                .bind(now_ms)
                .execute(&self.pool)
        )?;
        if res.rows_affected() == 0 {
            return Ok(None);
        }
        let row: Option<(String,)> = db_retry!(
            sqlx::query_as("SELECT user_id FROM weixin_bind_codes WHERE code = ?")
                .bind(code)
                .fetch_optional(&self.pool)
        )?;
        Ok(row.map(|r| r.0))
    }

    // Weixin chats
    pub async fn get_weixin_chat(&self, binding_id: &str) -> Result<Option<String>> {
        let row: Option<(String,)> = db_retry!(
            sqlx::query_as("SELECT session_id FROM weixin_chats WHERE binding_id = ?")
                .bind(binding_id)
                .fetch_optional(&self.pool)
        )?;
        Ok(row.map(|r| r.0))
    }

    pub async fn set_weixin_chat(&self, binding_id: &str, session_id: &str) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        db_retry!(
            sqlx::query(
                "INSERT INTO weixin_chats (id, binding_id, session_id, created_at) VALUES (?, ?, ?, ?) ON DUPLICATE KEY UPDATE session_id = VALUES(session_id)"
            )
            .bind(&id)
            .bind(binding_id)
            .bind(session_id)
            .bind(now)
            .execute(&self.pool)
        )?;
        Ok(())
    }

    // Client agents
    pub async fn upsert_client_agent(&self, id: &str, user_id: &str, hostname: Option<&str>, work_dir: Option<&str>, accept_remote: bool) -> Result<()> {
        db_retry!(
            sqlx::query(
                "INSERT INTO client_agents (id, user_id, hostname, work_dir, accept_remote) VALUES (?, ?, ?, ?, ?)
                 ON DUPLICATE KEY UPDATE user_id = VALUES(user_id), hostname = VALUES(hostname), work_dir = VALUES(work_dir), accept_remote = VALUES(accept_remote), updated_at = NOW()"
            )
            .bind(id)
            .bind(user_id)
            .bind(hostname)
            .bind(work_dir)
            .bind(accept_remote)
            .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn get_client_agent(&self, user_id: &str, client_id: &str) -> Result<Option<ClientAgent>> {
        let agent = db_retry!(
            sqlx::query_as::<_, ClientAgent>(
                "SELECT id, user_id, hostname, work_dir, accept_remote, created_at, updated_at FROM client_agents WHERE user_id = ? AND id = ?"
            )
            .bind(user_id)
            .bind(client_id)
            .fetch_optional(&self.pool)
        )?;
        Ok(agent)
    }

    pub async fn list_client_agents(&self, user_id: &str) -> Result<Vec<ClientAgent>> {
        let agents = db_retry!(
            sqlx::query_as::<_, ClientAgent>(
                "SELECT id, user_id, hostname, work_dir, accept_remote, created_at, updated_at FROM client_agents WHERE user_id = ? ORDER BY created_at ASC"
            )
            .bind(user_id)
            .fetch_all(&self.pool)
        )?;
        Ok(agents)
    }

    /// admin 用：不按用户过滤，列出全部 client agent
    pub async fn list_all_client_agents(&self) -> Result<Vec<ClientAgent>> {
        let agents = db_retry!(
            sqlx::query_as::<_, ClientAgent>(
                "SELECT id, user_id, hostname, work_dir, accept_remote, created_at, updated_at FROM client_agents ORDER BY created_at ASC"
            )
            .fetch_all(&self.pool)
        )?;
        Ok(agents)
    }

    /// admin 用：仅按 client_id 查询（dispatch 需要取出 user_id）
    pub async fn get_client_agent_by_id(&self, client_id: &str) -> Result<Option<ClientAgent>> {
        let agent = db_retry!(
            sqlx::query_as::<_, ClientAgent>(
                "SELECT id, user_id, hostname, work_dir, accept_remote, created_at, updated_at FROM client_agents WHERE id = ?"
            )
            .bind(client_id)
            .fetch_optional(&self.pool)
        )?;
        Ok(agent)
    }

    // Switch a session between server-local execution (None) and a remote client
    pub async fn set_session_exec_client(&self, session_id: &str, exec_client_id: Option<&str>, work_dir: Option<&str>) -> Result<()> {
        db_retry!(
            sqlx::query("UPDATE sessions SET exec_client_id = ?, work_dir = ?, updated_at = NOW() WHERE id = ?")
                .bind(exec_client_id)
                .bind(work_dir)
                .bind(session_id)
                .execute(&self.pool)
        )?;
        Ok(())
    }
}

/// Extract a text preview from JSON content (first text block, up to max_chars).
fn extract_preview(content_json: &str, max_chars: usize) -> String {
    if let Ok(blocks) = serde_json::from_str::<Vec<serde_json::Value>>(content_json) {
        for block in &blocks {
            // Only extract from direct text blocks: { "type": "text", "text": "..." }
            // Skip tool_result blocks — they are not user-authored content
            if block.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                continue;
            }
            if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                let preview: String = text.chars().take(max_chars).collect();
                return preview;
            }
        }
    }
    String::new()
}
