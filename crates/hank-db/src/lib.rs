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
                    tracing::warn!(
                        "DB connection error (attempt {}/{}), retrying in {}ms: {}",
                        attempts,
                        MAX_RETRIES,
                        delay_ms,
                        e
                    );
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

/// 外部 CLI Agent（codex / claude）的一份命名凭据配置（agent_cli_profiles 表）。
/// 每个后端可存多份（不同第三方中转、不同模型），同时只有一份 is_active。
/// 换端点时切换 active 即可，不必重新粘贴凭据，也不用登服务器改 agent-cli.env。
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AgentCliProfileRecord {
    pub id: String,
    pub backend: String,
    /// 便于识别的名字，如「penguinapi」「官方」。同后端内唯一。
    pub name: String,
    /// 凭据注入用的环境变量名。第三方 Anthropic 中转多数要 ANTHROPIC_AUTH_TOKEN
    /// 而不是 ANTHROPIC_API_KEY，两者不能混用，所以必须显式记录。
    pub auth_kind: String,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    /// 其余白名单环境变量的 JSON 对象，如 ANTHROPIC_DEFAULT_OPUS_MODEL。
    pub extra_env: String,
    /// 该后端当前启用的是哪一份。每个 backend 至多一行为 true。
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,
}

/// 飞书话题=会话映射（feishu_chats 表）
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct FeishuChat {
    pub id: String,
    pub account_id: String,
    pub chat_id: String,
    pub topic_id: String,
    pub session_id: String,
    pub user_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 飞书发起的 monorepo 部署任务。任务状态落库，server 重启后可继续恢复和通知。
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Deployment {
    pub id: String,
    pub session_id: String,
    pub user_id: String,
    pub account_id: String,
    pub chat_id: String,
    pub topic_id: String,
    pub source_dir: String,
    pub commit_sha: String,
    /// JSON 字符串，内容是受影响的固定部署目标数组。
    pub targets: String,
    pub summary: String,
    pub status: String,
    pub card_message_id: Option<String>,
    pub approved_by: Option<String>,
    pub approval_expires_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub result: Option<String>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Agent 交互单：确认闸门 / ask_user / 任务闸门的统一载体。
///
/// 为什么不能寄生在 session 上：此前 quant_confirm 存进程内 map、ask_user 存
/// sessions.pending_ask_user，都以 session_id 为 key。飞书话题一旦因 reuse policy
/// 判 Recreate 而重建 session，待确认单与授权立刻成为孤儿——用户点了确认却永远
/// 不会执行。交互单有自己的主键；恢复执行所需上下文冻结在 resume_ref 里，
/// session 怎么变都不影响定位。
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AgentInteraction {
    /// 交互单主键，即卡片上展示的「任务编号」
    pub id: String,
    /// 关联会话，仅用于回溯与派发，不再是身份来源
    pub session_id: String,
    pub user_id: String,
    pub channel: String,
    pub account_id: Option<String>,
    pub chat_id: Option<String>,
    pub topic_id: Option<String>,
    /// quant_confirm / ask_user（task_gate 留给后续任务闸门）
    pub kind: String,
    pub title: String,
    pub goal: Option<String>,
    pub analysis: Option<String>,
    /// 按钮文案数组 JSON，如 `["确认","否"]`（`options` 在部分 MySQL 版本敏感，查询时加反引号）
    pub options: String,
    /// pending / answered / executing / done / failed / expired / cancelled
    pub status: String,
    pub answer: Option<String>,
    pub answered_by: Option<String>,
    pub answered_at: Option<DateTime<Utc>>,
    /// 恢复执行所需上下文 JSON。quant_confirm / ask_user 存
    /// `{"tool_use_id":"…","source":"…","question":"…"}`，不从 session metadata 现读。
    pub resume_ref: Option<String>,
    pub card_message_id: Option<String>,
    /// NULL = 不过期。微信写 now+5min；飞书与网页写 NULL（5 分钟 TTL 是微信渠道特性）
    pub expires_at: Option<DateTime<Utc>>,
    pub result: Option<String>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 创建交互单的入参（字段多，避免位置参数触发 too_many_arguments）。
#[derive(Debug, Clone)]
pub struct NewInteraction<'a> {
    pub session_id: &'a str,
    pub user_id: &'a str,
    pub channel: &'a str,
    pub account_id: Option<&'a str>,
    pub chat_id: Option<&'a str>,
    pub topic_id: Option<&'a str>,
    pub kind: &'a str,
    pub title: &'a str,
    pub goal: Option<&'a str>,
    pub analysis: Option<&'a str>,
    /// 已序列化的 options JSON 文本
    pub options: &'a str,
    /// 已序列化的 resume_ref JSON 文本
    pub resume_ref: Option<&'a str>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// 团队任务流水线的运行时配置。存在 settings 表的单行 JSON 里
/// （key = 'team_task_config'），admin 可改、改完即时生效。
///
/// 为什么用一行 JSON 而不是每个开关一个 settings key：
/// 这几个字段有互相约束（enabled=true 要求 task_gate_enabled=true、
/// roles 与 gates 取值要匹配），分成多行会出现「改了一半」的中间态。
/// 单行 JSON 保证一次写入是原子的。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TeamTaskSettings {
    /// 两阶段闸门总开关（原 [server_agent].task_gate_enabled）
    pub task_gate_enabled: bool,
    /// 多角色流水线总开关（原 [team_task].enabled）
    pub enabled: bool,
    pub roles: Vec<String>,
    pub gates: Vec<String>,
    pub max_dev_rounds: i32,
    pub dashboard_base_url: Option<String>,
    /// 最后修改人（admin 用户名），审计用
    pub updated_by: Option<String>,
}

/// settings 表里存团队任务配置的 key。
pub const TEAM_TASK_SETTINGS_KEY: &str = "team_task_config";

/// 团队任务：串起开发/评审/测试多角色轮次的任务主体。
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TeamTask {
    pub id: String,
    /// 人类可读短编号 tsk_xxx_xxxx，飞书卡片与看板深链都用它
    pub task_no: String,
    pub session_id: String,
    pub user_id: String,
    /// feishu / dashboard（后续接 issue 巡检时再加取值）
    pub source: String,
    /// 外部 issue 标识，如 IK5MOR；本期不巡检，仅从飞书原文 #KEY 解析或看板手填
    pub issue_key: Option<String>,
    pub title: String,
    pub goal: Option<String>,
    /// 分析轮产出的四段 markdown。与 agent_interactions.analysis 同源冗余一份，
    /// 便于看板与后续角色 prompt 直接取用，不必回查交互单。
    pub analysis: Option<String>,
    /// pending_confirm / running_developer / pending_review_gate / running_reviewer
    /// / pending_dev_gate / running_tester / done / failed / cancelled
    pub status: String,
    /// developer / reviewer / tester；终态时为 None
    pub current_role: Option<String>,
    /// 开发轮已用轮次，用于 max_dev_rounds 上限判定
    pub dev_rounds: i32,
    /// 执行后端与节点在整条流水线内固定，中途不换节点
    pub backend: String,
    pub exec_client_id: Option<String>,
    pub agent_kind: String,
    pub account_id: Option<String>,
    pub chat_id: Option<String>,
    pub topic_id: Option<String>,
    /// 飞书任务主卡：跨角色复用同一张卡片原地刷新
    pub card_message_id: Option<String>,
    /// 闸门卡片的 message_id。主卡要 reply 一条已有消息，而建任务行时
    /// 还没有卡片（卡片是 pusher 收到 AskUser 后才发的），所以由 pusher
    /// 发闸门卡成功后回填此列，编排器派发首个角色时再 reply 生成主卡。
    pub origin_message_id: Option<String>,
    pub result: Option<String>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// 角色轮次：一个角色的一次执行。评审打回后重新开发会新增一行（round+1），历史保留。
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TeamTaskRun {
    pub id: String,
    pub task_id: String,
    /// developer / reviewer / tester
    pub role: String,
    /// 同一角色的第几轮，从 1 开始
    pub round: i32,
    /// 该角色本轮独占的 CLI thread；派发前为空，首个事件回来后写入
    pub thread_id: Option<String>,
    /// running / finished / failed / cancelled
    pub status: String,
    /// pass / reject / failed / unknown；unknown = 模型输出没解析出结论，需人工介入
    pub verdict: Option<String>,
    /// 结构化交接产物 JSON 文本（下一个角色的 prompt 输入）
    pub handoff: Option<String>,
    /// 该角色输出的正文摘要（看板展示）
    pub summary: Option<String>,
    /// 本轮新增改动文件数；查不到为 None
    pub dirty_files: Option<i32>,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// 任务级时间线事件：只记角色边界与人工决策，与 agent_events（单 run 细粒度）分开。
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TeamTaskEvent {
    pub id: i64,
    pub task_id: String,
    /// role_started / role_finished / gate_opened / gate_answered
    /// / rejected / status_changed / cancelled
    pub kind: String,
    pub role: Option<String>,
    pub round: Option<i32>,
    pub operator: Option<String>,
    pub detail: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// 创建团队任务的入参（字段多，避免位置参数触发 too_many_arguments）。
#[derive(Debug, Clone)]
pub struct NewTeamTask<'a> {
    pub session_id: &'a str,
    pub user_id: &'a str,
    pub source: &'a str,
    pub issue_key: Option<&'a str>,
    pub title: &'a str,
    pub goal: Option<&'a str>,
    pub analysis: Option<&'a str>,
    pub backend: &'a str,
    pub exec_client_id: Option<&'a str>,
    pub agent_kind: &'a str,
    pub account_id: Option<&'a str>,
    pub chat_id: Option<&'a str>,
    pub topic_id: Option<&'a str>,
}

/// 飞书自建应用账号（feishu_accounts 表，凭证由 admin REST 管理）
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct FeishuAccount {
    pub id: String,
    pub name: String,
    pub app_id: String,
    #[serde(skip_serializing)]
    pub app_secret: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 飞书用户 ↔ trace 用户绑定（feishu_bindings 表）
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct FeishuBinding {
    pub id: String,
    pub account_id: String,
    pub open_id: String,
    pub user_id: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct FeishuBindingWithUsername {
    pub id: String,
    pub account_id: String,
    pub open_id: String,
    pub user_id: String,
    pub username: String,
    pub created_at: DateTime<Utc>,
}

/// 渠道消息留档。该表不依赖渠道账号或用户外键，确保解绑/删除账号后历史仍可追溯。
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ChannelMessage {
    pub id: String,
    pub channel: String,
    pub account_id: String,
    pub account_name: String,
    pub conversation_id: String,
    pub topic_id: String,
    pub external_message_id: String,
    pub reply_to_external_id: Option<String>,
    pub direction: String,
    pub message_type: String,
    pub content: String,
    pub peer_id: Option<String>,
    pub user_id: Option<String>,
    pub username: Option<String>,
    pub session_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// 渠道会话列表项，由渠道消息按账号/聊天/话题聚合得到。
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ChannelConversation {
    pub channel: String,
    pub account_id: String,
    pub account_name: String,
    pub conversation_id: String,
    pub topic_id: String,
    pub peer_id: Option<String>,
    pub user_id: Option<String>,
    pub username: Option<String>,
    pub session_id: Option<String>,
    pub message_count: i64,
    pub first_message_at: DateTime<Utc>,
    pub last_message_at: DateTime<Utc>,
    pub last_direction: String,
    pub last_message_type: String,
    pub last_content: String,
    /// 该会话实际执行的后端（codex / claude / native provider 名），取自 sessions.provider。
    pub agent_provider: Option<String>,
    /// 实际使用的模型名，取自 sessions.model；旧会话没有记录时为空。
    pub agent_model: Option<String>,
}

/// 定时任务执行记录（job_runs 表，镜像 quant_job_run 模型）
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct JobRun {
    pub id: i64,
    pub job_id: String,
    /// system（调度器）| manual（admin 手动触发）
    pub trigger: String,
    /// running | finished | failed
    pub status: String,
    pub operator: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// 任务返回值的 JSON 文本
    pub result: Option<String>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
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
pub struct WeixinKimi {
    pub binding_id: String,
    pub client_id: Option<String>,
    pub term_id: Option<String>,
    pub work_dir: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ClientAgent {
    pub id: String,
    pub user_id: String,
    pub hostname: Option<String>,
    pub work_dir: Option<String>,
    pub accept_remote: bool,
    pub enabled: bool,
    pub last_active_at: Option<DateTime<Utc>>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ClientNotification {
    pub id: String,
    pub user_id: String,
    pub client_id: String,
    pub term_id: Option<String>,
    pub kind: String,
    pub title: String,
    pub body: Option<String>,
    pub created_at: DateTime<Utc>,
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
                payload LONGTEXT NOT NULL,
                seq BIGINT UNSIGNED NOT NULL DEFAULT 0,
                created_at DATETIME(6) NOT NULL DEFAULT NOW(6),
                FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
                INDEX idx_agent_events_session_seq (session_id, seq)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // 完整 LLM 请求会包含 system、messages、工具 schema，图片消息还可能带 base64。
        // 旧库从 MEDIUMTEXT 升为 LONGTEXT，确保审计事件不会因 16MB 上限静默丢失。
        let _ = sqlx::query("ALTER TABLE agent_events MODIFY COLUMN payload LONGTEXT NOT NULL")
            .execute(&pool)
            .await;

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
                created_at DATETIME NOT NULL DEFAULT NOW(),
                UNIQUE KEY uk_weixin_bot_id (ilink_bot_id)
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

        // 微信入站消息 claim：ilink 可能重复投递，多个 monitor 也不能重复执行同一消息。
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS weixin_inbound_messages (
                ilink_bot_id VARCHAR(128) NOT NULL,
                message_id BIGINT UNSIGNED NOT NULL,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                PRIMARY KEY (ilink_bot_id, message_id),
                INDEX idx_weixin_inbound_created (created_at)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Weixin kimi managed sessions table (binding → client 上托管的 Kimi CLI 终端)
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS weixin_kimi (
                binding_id VARCHAR(36) PRIMARY KEY,
                client_id VARCHAR(36) DEFAULT NULL,
                term_id VARCHAR(64) DEFAULT NULL,
                work_dir VARCHAR(512) DEFAULT NULL,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                updated_at DATETIME NOT NULL DEFAULT NOW(),
                FOREIGN KEY (binding_id) REFERENCES weixin_bindings(id) ON DELETE CASCADE
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Feishu accounts table（自建应用凭证，admin REST 管理，与 weixin_accounts 同模式）
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS feishu_accounts (
                id VARCHAR(36) PRIMARY KEY,
                name VARCHAR(128) NOT NULL DEFAULT '',
                app_id VARCHAR(64) NOT NULL,
                app_secret VARCHAR(128) NOT NULL,
                enabled BOOLEAN NOT NULL DEFAULT TRUE,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                updated_at DATETIME NOT NULL DEFAULT NOW(),
                UNIQUE KEY uk_feishu_app_id (app_id)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Feishu bindings table（飞书 open_id ↔ trace user，bind code 流程建立）
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS feishu_bindings (
                id VARCHAR(36) PRIMARY KEY,
                account_id VARCHAR(36) NOT NULL,
                open_id VARCHAR(64) NOT NULL,
                user_id VARCHAR(36) NOT NULL,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                FOREIGN KEY (account_id) REFERENCES feishu_accounts(id) ON DELETE CASCADE,
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
                UNIQUE KEY uk_feishu_binding (account_id, open_id)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Feishu bind codes table（一次性绑定码，与 weixin_bind_codes 同模式）
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS feishu_bind_codes (
                code VARCHAR(8) PRIMARY KEY,
                user_id VARCHAR(36) NOT NULL,
                expires_at BIGINT NOT NULL,
                used_at BIGINT DEFAULT NULL,
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Feishu chats table（话题=会话映射：account_id+chat_id+topic_id → session_id）
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS feishu_chats (
                id VARCHAR(36) PRIMARY KEY,
                account_id VARCHAR(36) NOT NULL,
                chat_id VARCHAR(64) NOT NULL,
                topic_id VARCHAR(64) NOT NULL,
                session_id VARCHAR(36) NOT NULL,
                user_id VARCHAR(36) NOT NULL,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                updated_at DATETIME NOT NULL DEFAULT NOW(),
                FOREIGN KEY (account_id) REFERENCES feishu_accounts(id) ON DELETE CASCADE,
                UNIQUE KEY uk_feishu_chat_topic (account_id, chat_id, topic_id)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // 飞书发起的代码部署任务。独立部署进程会跨越 hank-server 自身重启，
        // 因此审批、目标和终态必须持久化。
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS deployments (
                id VARCHAR(36) PRIMARY KEY,
                session_id VARCHAR(36) NOT NULL,
                user_id VARCHAR(36) NOT NULL,
                account_id VARCHAR(36) NOT NULL,
                chat_id VARCHAR(128) NOT NULL,
                topic_id VARCHAR(128) NOT NULL DEFAULT 'main',
                source_dir VARCHAR(1024) NOT NULL,
                commit_sha VARCHAR(64) NOT NULL,
                targets JSON NOT NULL,
                summary TEXT NOT NULL,
                status VARCHAR(32) NOT NULL DEFAULT 'awaiting_approval',
                card_message_id VARCHAR(256) DEFAULT NULL,
                approved_by VARCHAR(36) DEFAULT NULL,
                approval_expires_at DATETIME NOT NULL,
                started_at DATETIME DEFAULT NULL,
                finished_at DATETIME DEFAULT NULL,
                result MEDIUMTEXT DEFAULT NULL,
                error TEXT DEFAULT NULL,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                updated_at DATETIME NOT NULL DEFAULT NOW(),
                INDEX idx_deployments_status (status, updated_at),
                INDEX idx_deployments_session (session_id, created_at)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Agent 交互单：确认闸门 / ask_user / 任务闸门的统一载体。
        //
        // 为什么要落表：此前 quant_confirm 存进程内 map、ask_user 存 sessions 字段，
        // 两者都以 session_id 为 key。飞书话题会话一旦重建（reuse policy 判 Recreate），
        // 待确认单与授权立刻成为孤儿，用户点了确认却永远不会执行。交互单有自己的主键，
        // 恢复执行所需上下文冻结在 resume_ref 里，session 怎么变都不影响。
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS agent_interactions (
                id VARCHAR(36) PRIMARY KEY,
                session_id VARCHAR(36) NOT NULL,
                user_id VARCHAR(36) NOT NULL,
                channel VARCHAR(32) NOT NULL,
                account_id VARCHAR(36) DEFAULT NULL,
                chat_id VARCHAR(128) DEFAULT NULL,
                topic_id VARCHAR(128) DEFAULT NULL,
                kind VARCHAR(32) NOT NULL,
                title VARCHAR(255) NOT NULL,
                goal TEXT DEFAULT NULL,
                analysis MEDIUMTEXT DEFAULT NULL,
                `options` JSON NOT NULL,
                status VARCHAR(32) NOT NULL DEFAULT 'pending',
                answer VARCHAR(64) DEFAULT NULL,
                answered_by VARCHAR(36) DEFAULT NULL,
                answered_at DATETIME DEFAULT NULL,
                resume_ref JSON DEFAULT NULL,
                card_message_id VARCHAR(256) DEFAULT NULL,
                expires_at DATETIME DEFAULT NULL,
                result MEDIUMTEXT DEFAULT NULL,
                error TEXT DEFAULT NULL,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                updated_at DATETIME NOT NULL DEFAULT NOW(),
                INDEX idx_ai_status (status, updated_at),
                INDEX idx_ai_session (session_id, created_at),
                INDEX idx_ai_user (user_id, created_at)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // 渠道聊天记录留档（当前接入飞书，后续微信等渠道复用）。不建立账号/用户外键，
        // 保留应用删除、解绑后的审计快照；external_message_id 唯一键也用于入站去重。
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS channel_messages (
                id VARCHAR(36) PRIMARY KEY,
                channel VARCHAR(32) NOT NULL,
                account_id VARCHAR(36) NOT NULL,
                account_name VARCHAR(128) NOT NULL DEFAULT '',
                conversation_id VARCHAR(128) NOT NULL,
                topic_id VARCHAR(128) NOT NULL DEFAULT 'main',
                external_message_id VARCHAR(256) NOT NULL,
                reply_to_external_id VARCHAR(256) DEFAULT NULL,
                direction VARCHAR(16) NOT NULL,
                message_type VARCHAR(32) NOT NULL DEFAULT 'text',
                content MEDIUMTEXT NOT NULL,
                peer_id VARCHAR(128) DEFAULT NULL,
                user_id VARCHAR(36) DEFAULT NULL,
                username VARCHAR(128) DEFAULT NULL,
                session_id VARCHAR(36) DEFAULT NULL,
                created_at DATETIME(6) NOT NULL DEFAULT NOW(6),
                UNIQUE KEY uk_channel_external_message (channel, account_id, external_message_id),
                INDEX idx_channel_conversation (channel, account_id, conversation_id, topic_id, created_at),
                INDEX idx_channel_created (channel, created_at)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Job runs table（定时任务执行日志，镜像 quant_job_run：旁路日志，写失败不影响任务）
        // 注意 trigger 是 MySQL 保留字，必须带反引号
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS job_runs (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                job_id VARCHAR(64) NOT NULL,
                `trigger` VARCHAR(16) NOT NULL,
                status VARCHAR(16) NOT NULL,
                operator VARCHAR(64) DEFAULT NULL,
                started_at DATETIME NOT NULL,
                finished_at DATETIME DEFAULT NULL,
                result MEDIUMTEXT DEFAULT NULL,
                error TEXT DEFAULT NULL,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                INDEX idx_job_runs_job (job_id, id)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Job states table（定时任务启停开关；任务定义在代码里，状态在 DB）
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS job_states (
                job_id VARCHAR(64) PRIMARY KEY,
                enabled BOOLEAN NOT NULL DEFAULT TRUE,
                updated_at DATETIME NOT NULL DEFAULT NOW()
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
                enabled BOOLEAN NOT NULL DEFAULT TRUE,
                last_active_at DATETIME DEFAULT NULL,
                last_seen_at DATETIME DEFAULT NULL,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                updated_at DATETIME NOT NULL DEFAULT NOW(),
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
                INDEX idx_client_agents_user (user_id)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Client notifications table (终端通知上报：kimi task complete / approval 等)
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS client_notifications (
                id VARCHAR(36) PRIMARY KEY,
                user_id VARCHAR(36) NOT NULL,
                client_id VARCHAR(36) NOT NULL,
                term_id VARCHAR(64) DEFAULT NULL,
                kind VARCHAR(32) NOT NULL DEFAULT 'notification',
                title VARCHAR(255) NOT NULL DEFAULT '',
                body TEXT DEFAULT NULL,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                INDEX idx_client_notifications_user (user_id),
                INDEX idx_client_notifications_created (created_at)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // Migrations for existing databases
        // Add pushed_at to client_notifications (微信推送消费标记)
        let _ = sqlx::query(
            "ALTER TABLE client_notifications ADD COLUMN pushed_at DATETIME DEFAULT NULL AFTER created_at"
        ).execute(&pool).await;
        // Add category column to prompt_templates if not exists
        let _ = sqlx::query(
            "ALTER TABLE prompt_templates ADD COLUMN category VARCHAR(32) NOT NULL DEFAULT 'prompt' AFTER content"
        ).execute(&pool).await;
        let _ = sqlx::query(
            "ALTER TABLE prompt_templates ADD INDEX idx_prompt_templates_category (category)",
        )
        .execute(&pool)
        .await;
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
        // client_agents：节点停用开关与活跃时间（停用只挡自动选路，不挡 admin 代理）
        Self::ensure_column(
            &pool,
            "client_agents",
            "enabled",
            "ALTER TABLE client_agents ADD COLUMN enabled BOOLEAN NOT NULL DEFAULT TRUE AFTER accept_remote",
        )
        .await?;
        Self::ensure_column(
            &pool,
            "client_agents",
            "last_active_at",
            "ALTER TABLE client_agents ADD COLUMN last_active_at DATETIME DEFAULT NULL AFTER enabled",
        )
        .await?;
        Self::ensure_column(
            &pool,
            "client_agents",
            "last_seen_at",
            "ALTER TABLE client_agents ADD COLUMN last_seen_at DATETIME DEFAULT NULL AFTER last_active_at",
        )
        .await?;
        // 为旧库补 bot 唯一键；历史重复记录会使这条 DDL 失败，但运行时仍由
        // create_weixin_account 的查询和 monitor 的 bot 级去重兼容处理。
        let _ = sqlx::query(
            "ALTER TABLE weixin_accounts ADD UNIQUE KEY uk_weixin_bot_id (ilink_bot_id)",
        )
        .execute(&pool)
        .await;

        // 旧飞书会话建表时 provider/model 写的是空串，实际后端只存在
        // metadata.agent_backend 里，admin 列表因此分不出 codex / claude。
        // 这里按 metadata 回填 provider；model 无从追溯，留空由前端回退展示。
        for backend in ["codex", "claude"] {
            let _ = sqlx::query(
                "UPDATE sessions SET provider = ? \
                 WHERE provider = '' AND metadata LIKE ?",
            )
            .bind(backend)
            .bind(format!("%\"agent_backend\":\"{backend}\"%"))
            .execute(&pool)
            .await;
        }

        // 外部 CLI Agent 凭据：每个后端可存多份命名配置，同时只启用一份（is_active）。
        // 没有启用行时由 agent-cli.env 兜底，保留登服务器改文件的应急路径。
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS agent_cli_profiles (
                id VARCHAR(36) PRIMARY KEY,
                backend VARCHAR(16) NOT NULL,
                name VARCHAR(64) NOT NULL,
                auth_kind VARCHAR(32) NOT NULL DEFAULT '',
                api_key VARCHAR(512) NOT NULL DEFAULT '',
                base_url VARCHAR(512) NOT NULL DEFAULT '',
                model VARCHAR(128) NOT NULL DEFAULT '',
                extra_env TEXT NOT NULL,
                is_active BOOLEAN NOT NULL DEFAULT FALSE,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                updated_at DATETIME NOT NULL DEFAULT NOW(),
                updated_by VARCHAR(128) NOT NULL DEFAULT '',
                UNIQUE KEY uk_agent_cli_backend_name (backend, name),
                INDEX idx_agent_cli_active (backend, is_active)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // 团队任务：一个飞书话题任务的主体，串起开发/评审/测试多个角色轮次。
        // session_id 不加外键——与 channel_messages 同理，session 清理后仍要保留任务审计快照。
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS team_tasks (
                id VARCHAR(36) PRIMARY KEY,
                task_no VARCHAR(32) NOT NULL,
                session_id VARCHAR(36) NOT NULL,
                user_id VARCHAR(36) NOT NULL,
                source VARCHAR(32) NOT NULL DEFAULT 'feishu',
                issue_key VARCHAR(64) DEFAULT NULL,
                title VARCHAR(255) NOT NULL DEFAULT '',
                goal TEXT DEFAULT NULL,
                analysis MEDIUMTEXT DEFAULT NULL,
                status VARCHAR(32) NOT NULL DEFAULT 'pending_confirm',
                current_role VARCHAR(32) DEFAULT NULL,
                dev_rounds INT NOT NULL DEFAULT 0,
                backend VARCHAR(32) NOT NULL,
                exec_client_id VARCHAR(36) DEFAULT NULL,
                agent_kind VARCHAR(32) NOT NULL DEFAULT 'general_task',
                account_id VARCHAR(36) DEFAULT NULL,
                chat_id VARCHAR(128) DEFAULT NULL,
                topic_id VARCHAR(128) DEFAULT NULL,
                card_message_id VARCHAR(256) DEFAULT NULL,
                result MEDIUMTEXT DEFAULT NULL,
                error TEXT DEFAULT NULL,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                updated_at DATETIME NOT NULL DEFAULT NOW(),
                finished_at DATETIME DEFAULT NULL,
                UNIQUE KEY uk_team_tasks_no (task_no),
                INDEX idx_team_tasks_status (status, updated_at),
                INDEX idx_team_tasks_session (session_id, created_at),
                INDEX idx_team_tasks_user (user_id, created_at)
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // 角色轮次：一个角色的一次执行。评审打回后重新开发会新增一行（round+1），
        // 历史轮次全部保留，不覆盖。
        // uk_team_run_role_round 是并发防线：编排器重复派发同一角色同一轮会插入失败，
        // 而不是起两个并发 run（与进程内 TaskRegistry 名额互补，后者防同 session 并发）。
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS team_task_runs (
                id VARCHAR(36) PRIMARY KEY,
                task_id VARCHAR(36) NOT NULL,
                role VARCHAR(32) NOT NULL,
                round INT NOT NULL DEFAULT 1,
                thread_id VARCHAR(128) DEFAULT NULL,
                status VARCHAR(16) NOT NULL DEFAULT 'running',
                verdict VARCHAR(16) DEFAULT NULL,
                handoff JSON DEFAULT NULL,
                summary MEDIUMTEXT DEFAULT NULL,
                dirty_files INT DEFAULT NULL,
                error TEXT DEFAULT NULL,
                started_at DATETIME NOT NULL DEFAULT NOW(),
                finished_at DATETIME DEFAULT NULL,
                UNIQUE KEY uk_team_run_role_round (task_id, role, round),
                INDEX idx_team_runs_task (task_id, started_at),
                FOREIGN KEY (task_id) REFERENCES team_tasks(id) ON DELETE CASCADE
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // 任务级时间线：只记角色边界与人工决策，一个任务通常十几行。
        // 与 agent_events（单 run 内的细粒度事件）分开，避免看板读一次时间线拉出上万行。
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS team_task_events (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                task_id VARCHAR(36) NOT NULL,
                kind VARCHAR(32) NOT NULL,
                role VARCHAR(32) DEFAULT NULL,
                round INT DEFAULT NULL,
                operator VARCHAR(64) DEFAULT NULL,
                detail TEXT DEFAULT NULL,
                created_at DATETIME NOT NULL DEFAULT NOW(),
                INDEX idx_team_events_task (task_id, id),
                FOREIGN KEY (task_id) REFERENCES team_tasks(id) ON DELETE CASCADE
            ) DEFAULT CHARSET=utf8mb4",
        )
        .execute(&pool)
        .await?;

        // 闸门卡片 message_id：主卡 reply 目标。建任务行时还没有卡片，
        // 由 pusher 发闸门卡后回填；已有库用 ensure_column 幂等加列。
        Self::ensure_column(
            &pool,
            "team_tasks",
            "origin_message_id",
            "ALTER TABLE team_tasks ADD COLUMN origin_message_id VARCHAR(256) DEFAULT NULL",
        )
        .await?;

        // 从单行结构（agent_cli_configs）迁移到多配置。旧表每后端至多一行，
        // 迁成名为「默认」的配置并沿用原 enabled 作为 is_active。迁完删旧表，
        // 避免两份凭据并存。旧表不存在时（新库）整段静默跳过。
        let legacy_exists: Option<(String,)> = sqlx::query_as(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = DATABASE() AND table_name = 'agent_cli_configs'",
        )
        .fetch_optional(&pool)
        .await
        .unwrap_or(None);
        if legacy_exists.is_some() {
            let _ = sqlx::query(
                "INSERT IGNORE INTO agent_cli_profiles \
                 (id, backend, name, auth_kind, api_key, base_url, model, extra_env, \
                  is_active, created_at, updated_at, updated_by) \
                 SELECT UUID(), backend, '默认', auth_kind, api_key, base_url, model, \
                        extra_env, enabled, updated_at, updated_at, updated_by \
                 FROM agent_cli_configs",
            )
            .execute(&pool)
            .await;
            let _ = sqlx::query("DROP TABLE agent_cli_configs")
                .execute(&pool)
                .await;
        }

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
    pub async fn create_session(
        &self,
        provider: &str,
        model: &str,
        work_dir: Option<&str>,
        user_id: Option<&str>,
        environment: Option<&str>,
        session_type: Option<&str>,
        metadata: Option<&str>,
    ) -> Result<Session> {
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
        db_retry!(sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(id)
            .execute(&self.pool))?;
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
        db_retry!(sqlx::query(
            "UPDATE sessions SET work_dir = ?, updated_at = NOW() WHERE id = ?"
        )
        .bind(work_dir)
        .bind(id)
        .execute(&self.pool))?;
        Ok(())
    }

    /// 回写会话实际使用的 provider 与 model。外部 CLI Agent（codex / claude）建会话时
    /// 还不知道 CLI 最终选用哪个模型，首轮拿到模型名后用这个方法补上，admin 才能区分后端。
    pub async fn update_session_provider_model(
        &self,
        id: &str,
        provider: &str,
        model: &str,
    ) -> Result<()> {
        db_retry!(sqlx::query(
            "UPDATE sessions SET provider = ?, model = ?, updated_at = NOW() WHERE id = ?"
        )
        .bind(provider)
        .bind(model)
        .bind(id)
        .execute(&self.pool))?;
        Ok(())
    }

    pub async fn update_session_metadata(&self, id: &str, metadata: &str) -> Result<()> {
        db_retry!(sqlx::query(
            "UPDATE sessions SET metadata = ?, updated_at = NOW() WHERE id = ?"
        )
        .bind(metadata)
        .bind(id)
        .execute(&self.pool))?;
        Ok(())
    }

    pub async fn update_session_local_agent(
        &self,
        id: &str,
        local_agent: Option<&str>,
        local_work_dir: Option<&str>,
    ) -> Result<()> {
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
    pub async fn get_branch_messages(
        &self,
        session_id: &str,
        leaf_id: &str,
    ) -> Result<Vec<DbMessage>> {
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
        db_retry!(sqlx::query(
            "UPDATE sessions SET active_leaf_id = ?, updated_at = NOW() WHERE id = ?"
        )
        .bind(leaf_id)
        .bind(session_id)
        .execute(&self.pool))?;
        Ok(())
    }

    pub async fn truncate_messages(&self, session_id: &str, keep_count: u32) -> Result<u64> {
        // Get IDs of messages to keep (first N by ordering)
        let kept_ids: Vec<(String,)> = sqlx::query_as(
            "SELECT id FROM messages WHERE session_id = ? ORDER BY seq ASC, created_at ASC LIMIT ?",
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
        let row = db_retry!(sqlx::query_as::<_, Setting>(
            "SELECT `key`, value FROM settings WHERE `key` = ?"
        )
        .bind(key)
        .fetch_optional(&self.pool))?;
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

    // Agent CLI 凭据配置（codex / claude），每后端多份、同时启用一份
    const AGENT_CLI_COLUMNS: &'static str = "id, backend, name, auth_kind, api_key, base_url, \
         model, extra_env, is_active, created_at, updated_at, updated_by";

    pub async fn list_agent_cli_profiles(&self) -> Result<Vec<AgentCliProfileRecord>> {
        let sql = format!(
            "SELECT {} FROM agent_cli_profiles ORDER BY backend, name",
            Self::AGENT_CLI_COLUMNS
        );
        let rows =
            db_retry!(sqlx::query_as::<_, AgentCliProfileRecord>(&sql).fetch_all(&self.pool))?;
        Ok(rows)
    }

    pub async fn get_agent_cli_profile(&self, id: &str) -> Result<Option<AgentCliProfileRecord>> {
        let sql = format!(
            "SELECT {} FROM agent_cli_profiles WHERE id = ?",
            Self::AGENT_CLI_COLUMNS
        );
        let row = db_retry!(sqlx::query_as::<_, AgentCliProfileRecord>(&sql)
            .bind(id)
            .fetch_optional(&self.pool))?;
        Ok(row)
    }

    /// 取某后端当前启用的那份配置，cli_agent 每轮任务调用。
    pub async fn get_active_agent_cli_profile(
        &self,
        backend: &str,
    ) -> Result<Option<AgentCliProfileRecord>> {
        let sql = format!(
            "SELECT {} FROM agent_cli_profiles WHERE backend = ? AND is_active = TRUE LIMIT 1",
            Self::AGENT_CLI_COLUMNS
        );
        let row = db_retry!(sqlx::query_as::<_, AgentCliProfileRecord>(&sql)
            .bind(backend)
            .fetch_optional(&self.pool))?;
        Ok(row)
    }

    /// 新建一份命名配置。同后端内名字重复由 UNIQUE 约束拒绝。
    #[allow(clippy::too_many_arguments)]
    pub async fn create_agent_cli_profile(
        &self,
        backend: &str,
        name: &str,
        auth_kind: &str,
        api_key: &str,
        base_url: &str,
        model: &str,
        extra_env: &str,
        updated_by: &str,
    ) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        db_retry!(sqlx::query(
            "INSERT INTO agent_cli_profiles \
             (id, backend, name, auth_kind, api_key, base_url, model, extra_env, \
              is_active, created_at, updated_at, updated_by) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, FALSE, NOW(), NOW(), ?)"
        )
        .bind(&id)
        .bind(backend)
        .bind(name)
        .bind(auth_kind)
        .bind(api_key)
        .bind(base_url)
        .bind(model)
        .bind(extra_env)
        .bind(updated_by)
        .execute(&self.pool))?;
        Ok(id)
    }

    /// 更新一份配置。api_key 传空串表示保留库里已有的 key，避免每次改端点或模型
    /// 都要重新粘贴凭据。不改 is_active，切换启用走 activate_agent_cli_profile。
    #[allow(clippy::too_many_arguments)]
    pub async fn update_agent_cli_profile(
        &self,
        id: &str,
        name: &str,
        auth_kind: &str,
        api_key: &str,
        base_url: &str,
        model: &str,
        extra_env: &str,
        updated_by: &str,
    ) -> Result<()> {
        db_retry!(sqlx::query(
            "UPDATE agent_cli_profiles SET \
             name = ?, auth_kind = ?, \
             api_key = IF(? = '', api_key, ?), \
             base_url = ?, model = ?, extra_env = ?, \
             updated_at = NOW(), updated_by = ? \
             WHERE id = ?"
        )
        .bind(name)
        .bind(auth_kind)
        .bind(api_key)
        .bind(api_key)
        .bind(base_url)
        .bind(model)
        .bind(extra_env)
        .bind(updated_by)
        .bind(id)
        .execute(&self.pool))?;
        Ok(())
    }

    /// 启用某份配置，同后端其余自动停用。用单条 UPDATE 完成，避免「先清空再置位」
    /// 中间态被并发的任务读到「一份都没启用」而错误回退到 agent-cli.env。
    pub async fn activate_agent_cli_profile(&self, id: &str, updated_by: &str) -> Result<()> {
        db_retry!(sqlx::query(
            "UPDATE agent_cli_profiles \
             SET is_active = (id = ?), \
                 updated_at = IF(id = ?, NOW(), updated_at), \
                 updated_by = IF(id = ?, ?, updated_by) \
             WHERE backend = (SELECT backend FROM (SELECT backend FROM agent_cli_profiles \
                              WHERE id = ?) AS target)"
        )
        .bind(id)
        .bind(id)
        .bind(id)
        .bind(updated_by)
        .bind(id)
        .execute(&self.pool))?;
        Ok(())
    }

    /// 停用某后端的全部配置，回退到 agent-cli.env。
    pub async fn deactivate_agent_cli_profiles(&self, backend: &str) -> Result<()> {
        db_retry!(sqlx::query(
            "UPDATE agent_cli_profiles SET is_active = FALSE WHERE backend = ?"
        )
        .bind(backend)
        .execute(&self.pool))?;
        Ok(())
    }

    /// 删除一份配置，彻底清掉其中的凭据。停用只是不再使用，凭据仍留在库里；
    /// 轮换掉泄露的 key 时需要真正删除。
    pub async fn delete_agent_cli_profile(&self, id: &str) -> Result<()> {
        db_retry!(sqlx::query("DELETE FROM agent_cli_profiles WHERE id = ?")
            .bind(id)
            .execute(&self.pool))?;
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

    pub async fn get_session_tool_executions(
        &self,
        session_id: &str,
    ) -> Result<Vec<ToolExecution>> {
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

    pub async fn get_last_agent_event_seq(&self, session_id: &str) -> Result<u64> {
        let row: (Option<u64>,) = db_retry!(sqlx::query_as(
            "SELECT MAX(seq) FROM agent_events WHERE session_id = ?"
        )
        .bind(session_id)
        .fetch_one(&self.pool))?;
        Ok(row.0.unwrap_or(0))
    }

    // Prompt Templates
    pub async fn save_prompt_template(
        &self,
        name: &str,
        content: &str,
        category: Option<&str>,
    ) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        let cat = category.unwrap_or("prompt");
        // Get next version for this name
        let max_version: Option<(i32,)> =
            sqlx::query_as("SELECT COALESCE(MAX(version), 0) FROM prompt_templates WHERE name = ?")
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
        db_retry!(sqlx::query("DELETE FROM prompt_templates WHERE id = ?")
            .bind(id)
            .execute(&self.pool))?;
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

    pub async fn create_user(
        &self,
        username: &str,
        password: &str,
        can_admin: bool,
        can_client: bool,
    ) -> Result<User> {
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
        Ok(User {
            id,
            username: username.to_string(),
            password_hash: hash,
            can_login_admin: can_admin,
            can_login_client: can_client,
            created_at: now,
        })
    }

    pub async fn update_user_permissions(
        &self,
        id: &str,
        can_admin: bool,
        can_client: bool,
    ) -> Result<()> {
        db_retry!(sqlx::query(
            "UPDATE users SET can_login_admin = ?, can_login_client = ? WHERE id = ?"
        )
        .bind(can_admin)
        .bind(can_client)
        .bind(id)
        .execute(&self.pool))?;
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
        db_retry!(sqlx::query("DELETE FROM users WHERE id = ?")
            .bind(id)
            .execute(&self.pool))?;
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
            id,
            name: name.to_string(),
            provider_type: provider_type.to_string(),
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
            default_model: default_model.to_string(),
            models: models.to_string(),
            priority,
            enabled,
            created_at: now,
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
        db_retry!(sqlx::query("DELETE FROM providers WHERE id = ?")
            .bind(id)
            .execute(&self.pool))?;
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
                "INSERT INTO image_providers (id, name, provider_type, api_key, base_url, default_model, models, priority, enabled, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(&id).bind(name).bind(provider_type).bind(api_key)
            .bind(base_url).bind(default_model).bind(models).bind(priority).bind(enabled).bind(now)
            .execute(&self.pool)
        )?;
        Ok(ProviderRecord {
            id,
            name: name.to_string(),
            provider_type: provider_type.to_string(),
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
            default_model: default_model.to_string(),
            models: models.to_string(),
            priority,
            enabled,
            created_at: now,
        })
    }

    pub async fn update_image_provider(
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
                "UPDATE image_providers SET name=?, provider_type=?, api_key=?, base_url=?, default_model=?, models=?, priority=?, enabled=? WHERE id=?"
            )
            .bind(name).bind(provider_type).bind(api_key).bind(base_url)
            .bind(default_model).bind(models).bind(priority).bind(enabled).bind(id)
            .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn delete_image_provider(&self, id: &str) -> Result<()> {
        db_retry!(sqlx::query("DELETE FROM image_providers WHERE id = ?")
            .bind(id)
            .execute(&self.pool))?;
        Ok(())
    }

    pub async fn provider_count(&self) -> Result<i64> {
        let count: (i64,) =
            db_retry!(sqlx::query_as("SELECT COUNT(*) FROM providers").fetch_one(&self.pool))?;
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
            local_port,
            db_host,
            db_port,
            proxy_host,
            proxy_port
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
                        Self::handle_tunnel(client, &proxy_h, proxy_port, &target_h, target_port)
                            .await
                    {
                        tracing::error!("Tunnel connection error: {}", e);
                    }
                });
            }
        });

        // Rewrite the database URL to connect through the local tunnel
        let mut new_url = parsed.clone();
        new_url.set_host(Some("127.0.0.1"))?;
        new_url
            .set_port(Some(local_port))
            .map_err(|_| anyhow::anyhow!("failed to set port"))?;

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

    pub async fn create_spec(
        &self,
        capability: &str,
        title: &str,
        content: &str,
        metadata: Option<&str>,
    ) -> Result<Spec> {
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
        Ok(Spec {
            id,
            capability: capability.to_string(),
            title: title.to_string(),
            content: content.to_string(),
            metadata: metadata.map(|s| s.to_string()),
            version: 1,
            created_at: now,
            updated_at: now,
        })
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

    pub async fn update_spec(
        &self,
        id: &str,
        content: Option<&str>,
        metadata: Option<&str>,
        title: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now();
        if let Some(c) = content {
            db_retry!(sqlx::query(
                "UPDATE specs SET content = ?, updated_at = ?, version = version + 1 WHERE id = ?"
            )
            .bind(c)
            .bind(now)
            .bind(id)
            .execute(&self.pool))?;
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
        db_retry!(sqlx::query("DELETE FROM specs WHERE id = ?")
            .bind(id)
            .execute(&self.pool))?;
        Ok(())
    }

    // ─── Spec Versions ───────────────────────────────────────────────────

    pub async fn create_spec_version(
        &self,
        spec_id: &str,
        version: i32,
        content: &str,
        metadata: Option<&str>,
        change_id: Option<&str>,
    ) -> Result<String> {
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

    pub async fn create_change(
        &self,
        name: &str,
        work_dir: Option<&str>,
        requirement_path: Option<&str>,
        tasks_path: Option<&str>,
    ) -> Result<Change> {
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
        Ok(Change {
            id,
            name: name.to_string(),
            status: "draft".to_string(),
            work_dir: work_dir.map(|s| s.to_string()),
            explore_summary: None,
            requirement_path: requirement_path.map(|s| s.to_string()),
            tasks_path: tasks_path.map(|s| s.to_string()),
            created_at: now,
            updated_at: now,
            archived_at: None,
        })
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

    pub async fn update_change(
        &self,
        id: &str,
        name: Option<&str>,
        status: Option<&str>,
    ) -> Result<()> {
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
                db_retry!(sqlx::query(
                    "UPDATE changes SET status = ?, updated_at = ?, archived_at = ? WHERE id = ?"
                )
                .bind(s)
                .bind(now)
                .bind(now)
                .bind(id)
                .execute(&self.pool))?;
            } else {
                db_retry!(sqlx::query(
                    "UPDATE changes SET status = ?, updated_at = ? WHERE id = ?"
                )
                .bind(s)
                .bind(now)
                .bind(id)
                .execute(&self.pool))?;
            }
        }
        Ok(())
    }

    pub async fn delete_change(&self, id: &str) -> Result<()> {
        db_retry!(sqlx::query("DELETE FROM changes WHERE id = ?")
            .bind(id)
            .execute(&self.pool))?;
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

    pub async fn update_change_explore_summary(
        &self,
        change_id: &str,
        summary: &str,
    ) -> Result<()> {
        db_retry!(sqlx::query(
            "UPDATE changes SET explore_summary = ?, updated_at = NOW() WHERE id = ?"
        )
        .bind(summary)
        .bind(change_id)
        .execute(&self.pool))?;
        Ok(())
    }

    // ─── Session: change_id ─────────────────────────────────────────────
    // pending_ask_user 读写方法已删除：待确认状态迁到 agent_interactions 后
    // 零调用。列与 Session 字段仍保留（生产有数据，删列需 migration）。

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
        db_retry!(sqlx::query(
            "UPDATE sessions SET change_id = ?, updated_at = NOW() WHERE id = ?"
        )
        .bind(change_id)
        .bind(session_id)
        .execute(&self.pool))?;
        Ok(())
    }

    // ─── Change Artifacts ────────────────────────────────────────────────

    pub async fn create_artifact(
        &self,
        change_id: &str,
        artifact_type: &str,
        capability: Option<&str>,
        content: &str,
        metadata: Option<&str>,
        status: Option<&str>,
    ) -> Result<ChangeArtifact> {
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
        Ok(ChangeArtifact {
            id,
            change_id: change_id.to_string(),
            artifact_type: artifact_type.to_string(),
            capability: capability.map(|s| s.to_string()),
            content: content.to_string(),
            metadata: metadata.map(|s| s.to_string()),
            status: st.to_string(),
            created_at: now,
            updated_at: now,
        })
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

    pub async fn update_artifact(
        &self,
        id: &str,
        content: Option<&str>,
        metadata: Option<&str>,
        status: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now();
        if let Some(c) = content {
            db_retry!(sqlx::query(
                "UPDATE change_artifacts SET content = ?, updated_at = ? WHERE id = ?"
            )
            .bind(c)
            .bind(now)
            .bind(id)
            .execute(&self.pool))?;
        }
        if let Some(m) = metadata {
            db_retry!(sqlx::query(
                "UPDATE change_artifacts SET metadata = ?, updated_at = ? WHERE id = ?"
            )
            .bind(m)
            .bind(now)
            .bind(id)
            .execute(&self.pool))?;
        }
        if let Some(s) = status {
            db_retry!(sqlx::query(
                "UPDATE change_artifacts SET status = ?, updated_at = ? WHERE id = ?"
            )
            .bind(s)
            .bind(now)
            .bind(id)
            .execute(&self.pool))?;
        }
        Ok(())
    }

    pub async fn delete_artifact(&self, id: &str) -> Result<()> {
        db_retry!(sqlx::query("DELETE FROM change_artifacts WHERE id = ?")
            .bind(id)
            .execute(&self.pool))?;
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

    pub async fn batch_create_tasks(
        &self,
        change_id: &str,
        tasks: &[(String, i32, i32, String, Option<String>)],
    ) -> Result<Vec<ChangeTask>> {
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
                id,
                change_id: change_id.to_string(),
                group_name: group_name.clone(),
                group_order: *group_order,
                task_order: *task_order,
                title: title.clone(),
                description: description.clone(),
                status: "pending".to_string(),
                session_id: None,
                created_at: now,
                updated_at: now,
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

    pub async fn update_task(
        &self,
        id: &str,
        status: Option<&str>,
        title: Option<&str>,
        description: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now();
        if let Some(s) = status {
            db_retry!(sqlx::query(
                "UPDATE change_tasks SET status = ?, updated_at = ? WHERE id = ?"
            )
            .bind(s)
            .bind(now)
            .bind(id)
            .execute(&self.pool))?;
        }
        if let Some(t) = title {
            db_retry!(sqlx::query(
                "UPDATE change_tasks SET title = ?, updated_at = ? WHERE id = ?"
            )
            .bind(t)
            .bind(now)
            .bind(id)
            .execute(&self.pool))?;
        }
        if let Some(d) = description {
            db_retry!(sqlx::query(
                "UPDATE change_tasks SET description = ?, updated_at = ? WHERE id = ?"
            )
            .bind(d)
            .bind(now)
            .bind(id)
            .execute(&self.pool))?;
        }
        if let Some(sid) = session_id {
            db_retry!(sqlx::query(
                "UPDATE change_tasks SET session_id = ?, updated_at = ? WHERE id = ?"
            )
            .bind(sid)
            .bind(now)
            .bind(id)
            .execute(&self.pool))?;
        }
        Ok(())
    }

    pub async fn delete_task(&self, id: &str) -> Result<()> {
        db_retry!(sqlx::query("DELETE FROM change_tasks WHERE id = ?")
            .bind(id)
            .execute(&self.pool))?;
        Ok(())
    }

    pub async fn get_change_task_counts(&self, change_id: &str) -> Result<(i64, i64, i64, i64)> {
        let total: (i64,) = db_retry!(sqlx::query_as(
            "SELECT COUNT(*) FROM change_tasks WHERE change_id = ?"
        )
        .bind(change_id)
        .fetch_one(&self.pool))?;
        let done: (i64,) = db_retry!(sqlx::query_as(
            "SELECT COUNT(*) FROM change_tasks WHERE change_id = ? AND status = 'done'"
        )
        .bind(change_id)
        .fetch_one(&self.pool))?;
        let in_progress: (i64,) = db_retry!(sqlx::query_as(
            "SELECT COUNT(*) FROM change_tasks WHERE change_id = ? AND status = 'in_progress'"
        )
        .bind(change_id)
        .fetch_one(&self.pool))?;
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

    pub async fn delete_checkpoints_after(
        &self,
        session_id: &str,
        created_at: DateTime<Utc>,
    ) -> Result<()> {
        db_retry!(
            sqlx::query("DELETE FROM checkpoints WHERE session_id = ? AND created_at > ?")
                .bind(session_id)
                .bind(created_at)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    // ─── Requirement Docs ───────────────────────────────────────────────

    pub async fn create_requirement_doc(
        &self,
        change_id: &str,
        session_id: Option<&str>,
        name: &str,
        content: &str,
        progress_json: Option<&str>,
    ) -> Result<RequirementDoc> {
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
        Ok(RequirementDoc {
            id,
            change_id: change_id.to_string(),
            session_id: session_id.map(|s| s.to_string()),
            name: name.to_string(),
            content: content.to_string(),
            version: 1,
            progress_json: progress_json.map(|s| s.to_string()),
            status: "draft".to_string(),
            created_at: now,
            updated_at: now,
        })
    }

    pub async fn update_requirement_doc(
        &self,
        id: &str,
        content: &str,
        progress_json: Option<&str>,
        status: Option<&str>,
        source: &str,
    ) -> Result<()> {
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
        let row: (i32,) = db_retry!(sqlx::query_as(
            "SELECT version FROM requirement_docs WHERE id = ?"
        )
        .bind(id)
        .fetch_one(&self.pool))?;
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

    pub async fn get_requirement_doc_by_change(
        &self,
        change_id: &str,
    ) -> Result<Option<RequirementDoc>> {
        let doc = db_retry!(
            sqlx::query_as::<_, RequirementDoc>(
                "SELECT id, change_id, session_id, name, content, version, progress_json, status, created_at, updated_at FROM requirement_docs WHERE change_id = ? ORDER BY created_at DESC LIMIT 1"
            )
            .bind(change_id)
            .fetch_optional(&self.pool)
        )?;
        Ok(doc)
    }

    pub async fn list_requirement_docs(
        &self,
        search: Option<&str>,
        status: Option<&str>,
        page: u32,
        page_size: u32,
    ) -> Result<(Vec<RequirementDoc>, u64)> {
        let offset = (page.saturating_sub(1)) * page_size;
        let mut where_clauses = Vec::new();
        if search.is_some() {
            where_clauses
                .push("(name LIKE CONCAT('%', ?, '%') OR content LIKE CONCAT('%', ?, '%'))");
        }
        if status.is_some() {
            where_clauses.push("status = ?");
        }
        let where_sql = if where_clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", where_clauses.join(" AND "))
        };

        let count_sql = format!("SELECT COUNT(*) FROM requirement_docs {}", where_sql);
        let list_sql = format!("SELECT id, change_id, session_id, name, content, version, progress_json, status, created_at, updated_at FROM requirement_docs {} ORDER BY updated_at DESC LIMIT ? OFFSET ?", where_sql);

        // Build count query
        let mut count_q = sqlx::query_as::<_, (i64,)>(&count_sql);
        if let Some(s) = search {
            count_q = count_q.bind(s).bind(s);
        }
        if let Some(st) = status {
            count_q = count_q.bind(st);
        }
        let (total,): (i64,) = count_q.fetch_one(&self.pool).await?;

        // Build list query
        let mut list_q = sqlx::query_as::<_, RequirementDoc>(&list_sql);
        if let Some(s) = search {
            list_q = list_q.bind(s).bind(s);
        }
        if let Some(st) = status {
            list_q = list_q.bind(st);
        }
        list_q = list_q.bind(page_size).bind(offset);
        let docs = list_q.fetch_all(&self.pool).await?;

        Ok((docs, total as u64))
    }

    pub async fn list_all_tasks(
        &self,
        status: Option<&str>,
        change_id: Option<&str>,
        page: u32,
        page_size: u32,
    ) -> Result<(Vec<ChangeTask>, u64)> {
        let offset = (page.saturating_sub(1)) * page_size;
        let mut where_clauses = Vec::new();
        if status.is_some() {
            where_clauses.push("status = ?");
        }
        if change_id.is_some() {
            where_clauses.push("change_id = ?");
        }
        let where_sql = if where_clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", where_clauses.join(" AND "))
        };

        let count_sql = format!("SELECT COUNT(*) FROM change_tasks {}", where_sql);
        let list_sql = format!("SELECT id, change_id, group_name, group_order, task_order, title, description, status, session_id, created_at, updated_at FROM change_tasks {} ORDER BY created_at DESC LIMIT ? OFFSET ?", where_sql);

        let mut count_q = sqlx::query_as::<_, (i64,)>(&count_sql);
        if let Some(s) = status {
            count_q = count_q.bind(s);
        }
        if let Some(c) = change_id {
            count_q = count_q.bind(c);
        }
        let (total,): (i64,) = count_q.fetch_one(&self.pool).await?;

        let mut list_q = sqlx::query_as::<_, ChangeTask>(&list_sql);
        if let Some(s) = status {
            list_q = list_q.bind(s);
        }
        if let Some(c) = change_id {
            list_q = list_q.bind(c);
        }
        list_q = list_q.bind(page_size).bind(offset);
        let tasks = list_q.fetch_all(&self.pool).await?;

        Ok((tasks, total as u64))
    }

    // Weixin accounts
    pub async fn create_weixin_account(
        &self,
        ilink_bot_id: &str,
        bot_token: &str,
        base_url: &str,
        bot_user_id: Option<&str>,
    ) -> Result<String> {
        // 兼容尚未有 bot 唯一键的旧库，避免重复登录再次插入账号；新库由唯一键保护并发插入。
        let existing: Option<(String,)> = db_retry!(
            sqlx::query_as(
                "SELECT id FROM weixin_accounts WHERE ilink_bot_id = ? ORDER BY created_at DESC LIMIT 1"
            )
            .bind(ilink_bot_id)
            .fetch_optional(&self.pool)
        )?;
        if let Some((existing_id,)) = existing {
            db_retry!(
                sqlx::query(
                    "UPDATE weixin_accounts SET bot_token = ?, base_url = ?, bot_user_id = ?, enabled = TRUE WHERE id = ?"
                )
                .bind(bot_token)
                .bind(base_url)
                .bind(bot_user_id)
                .bind(&existing_id)
                .execute(&self.pool)
            )?;
            return Ok(existing_id);
        }

        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        db_retry!(
            sqlx::query(
                "INSERT INTO weixin_accounts (id, ilink_bot_id, bot_token, base_url, bot_user_id, enabled, created_at)
                 VALUES (?, ?, ?, ?, ?, TRUE, ?)
                 ON DUPLICATE KEY UPDATE bot_token = VALUES(bot_token), base_url = VALUES(base_url),
                    bot_user_id = VALUES(bot_user_id), enabled = TRUE"
            )
            .bind(&id)
            .bind(ilink_bot_id)
            .bind(bot_token)
            .bind(base_url)
            .bind(bot_user_id)
            .bind(now)
            .execute(&self.pool)
        )?;
        // 旧库可能已经有重复行，始终返回该 bot 最新的账号记录。
        let row: Option<(String,)> = db_retry!(
            sqlx::query_as(
                "SELECT id FROM weixin_accounts WHERE ilink_bot_id = ? ORDER BY created_at DESC LIMIT 1"
            )
            .bind(ilink_bot_id)
            .fetch_optional(&self.pool)
        )?;
        Ok(row.map(|r| r.0).unwrap_or(id))
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

    pub async fn update_weixin_token(
        &self,
        id: &str,
        bot_token: &str,
        base_url: &str,
    ) -> Result<()> {
        db_retry!(sqlx::query(
            "UPDATE weixin_accounts SET bot_token = ?, base_url = ? WHERE id = ?"
        )
        .bind(bot_token)
        .bind(base_url)
        .bind(id)
        .execute(&self.pool))?;
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
        db_retry!(sqlx::query("DELETE FROM weixin_accounts WHERE id = ?")
            .bind(id)
            .execute(&self.pool))?;
        Ok(())
    }

    // Weixin bindings
    pub async fn create_weixin_binding(
        &self,
        account_id: &str,
        ilink_user_id: &str,
        user_id: &str,
    ) -> Result<String> {
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

    pub async fn get_weixin_binding(
        &self,
        account_id: &str,
        ilink_user_id: &str,
    ) -> Result<Option<WeixinBinding>> {
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

    /// 按 bot 身份查绑定，兼容历史上同一个 bot 被重复落库的账号记录。
    pub async fn get_weixin_binding_by_bot(
        &self,
        ilink_bot_id: &str,
        ilink_user_id: &str,
    ) -> Result<Option<WeixinBinding>> {
        let row = db_retry!(sqlx::query_as::<_, WeixinBinding>(
            "SELECT b.id, b.account_id, b.ilink_user_id, b.user_id, b.context_token, b.created_at
                 FROM weixin_bindings b
                 JOIN weixin_accounts a ON a.id = b.account_id
                 WHERE a.ilink_bot_id = ? AND b.ilink_user_id = ?
                 ORDER BY b.created_at DESC LIMIT 1"
        )
        .bind(ilink_bot_id)
        .bind(ilink_user_id)
        .fetch_optional(&self.pool))?;
        Ok(row)
    }

    pub async fn get_weixin_binding_by_id(&self, id: &str) -> Result<Option<WeixinBinding>> {
        let row = db_retry!(
            sqlx::query_as::<_, WeixinBinding>(
                "SELECT id, account_id, ilink_user_id, user_id, context_token, created_at FROM weixin_bindings WHERE id = ?"
            )
            .bind(id)
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

    /// 原子领取一条微信入站消息。重复投递或重复 monitor 只允许首个调用继续处理。
    pub async fn claim_weixin_message(&self, ilink_bot_id: &str, message_id: u64) -> Result<bool> {
        let res = db_retry!(sqlx::query(
            "INSERT IGNORE INTO weixin_inbound_messages (ilink_bot_id, message_id) VALUES (?, ?)"
        )
        .bind(ilink_bot_id)
        .bind(message_id)
        .execute(&self.pool))?;
        Ok(res.rows_affected() == 1)
    }

    /// 入站消息只用于短期去重，定期删除过期 claim 避免表无限增长。
    pub async fn purge_old_weixin_messages(&self) -> Result<u64> {
        let res = db_retry!(sqlx::query(
            "DELETE FROM weixin_inbound_messages WHERE created_at < NOW() - INTERVAL 30 DAY"
        )
        .execute(&self.pool))?;
        Ok(res.rows_affected())
    }

    pub async fn delete_weixin_binding(&self, id: &str) -> Result<()> {
        db_retry!(sqlx::query("DELETE FROM weixin_bindings WHERE id = ?")
            .bind(id)
            .execute(&self.pool))?;
        Ok(())
    }

    // Weixin bind codes
    pub async fn create_weixin_bind_code(
        &self,
        code: &str,
        user_id: &str,
        expires_at: i64,
    ) -> Result<()> {
        db_retry!(sqlx::query(
            "INSERT INTO weixin_bind_codes (code, user_id, expires_at) VALUES (?, ?, ?)"
        )
        .bind(code)
        .bind(user_id)
        .bind(expires_at)
        .execute(&self.pool))?;
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
        let row: Option<(String,)> = db_retry!(sqlx::query_as(
            "SELECT user_id FROM weixin_bind_codes WHERE code = ?"
        )
        .bind(code)
        .fetch_optional(&self.pool))?;
        Ok(row.map(|r| r.0))
    }

    // Weixin chats
    pub async fn get_weixin_chat(&self, binding_id: &str) -> Result<Option<String>> {
        let row: Option<(String,)> = db_retry!(sqlx::query_as(
            "SELECT session_id FROM weixin_chats WHERE binding_id = ?"
        )
        .bind(binding_id)
        .fetch_optional(&self.pool))?;
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

    // Feishu accounts
    pub async fn create_feishu_account(
        &self,
        name: &str,
        app_id: &str,
        app_secret: &str,
    ) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        db_retry!(
            sqlx::query(
                "INSERT INTO feishu_accounts (id, name, app_id, app_secret, enabled, created_at, updated_at) VALUES (?, ?, ?, ?, TRUE, ?, ?)"
            )
            .bind(&id)
            .bind(name)
            .bind(app_id)
            .bind(app_secret)
            .bind(now)
            .bind(now)
            .execute(&self.pool)
        )?;
        Ok(id)
    }

    pub async fn list_feishu_accounts(&self) -> Result<Vec<FeishuAccount>> {
        let rows = db_retry!(
            sqlx::query_as::<_, FeishuAccount>(
                "SELECT id, name, app_id, app_secret, enabled, created_at, updated_at FROM feishu_accounts ORDER BY created_at ASC"
            )
            .fetch_all(&self.pool)
        )?;
        Ok(rows)
    }

    pub async fn get_feishu_account(&self, id: &str) -> Result<Option<FeishuAccount>> {
        let row = db_retry!(
            sqlx::query_as::<_, FeishuAccount>(
                "SELECT id, name, app_id, app_secret, enabled, created_at, updated_at FROM feishu_accounts WHERE id = ?"
            )
            .bind(id)
            .fetch_optional(&self.pool)
        )?;
        Ok(row)
    }

    pub async fn set_feishu_account_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        db_retry!(sqlx::query(
            "UPDATE feishu_accounts SET enabled = ?, updated_at = NOW() WHERE id = ?"
        )
        .bind(enabled)
        .bind(id)
        .execute(&self.pool))?;
        Ok(())
    }

    pub async fn update_feishu_account(
        &self,
        id: &str,
        name: &str,
        app_secret: Option<&str>,
    ) -> Result<()> {
        match app_secret {
            Some(secret) => {
                db_retry!(
                    sqlx::query("UPDATE feishu_accounts SET name = ?, app_secret = ?, updated_at = NOW() WHERE id = ?")
                        .bind(name)
                        .bind(secret)
                        .bind(id)
                        .execute(&self.pool)
                )?;
            }
            None => {
                db_retry!(sqlx::query(
                    "UPDATE feishu_accounts SET name = ?, updated_at = NOW() WHERE id = ?"
                )
                .bind(name)
                .bind(id)
                .execute(&self.pool))?;
            }
        }
        Ok(())
    }

    pub async fn delete_feishu_account(&self, id: &str) -> Result<()> {
        db_retry!(sqlx::query("DELETE FROM feishu_accounts WHERE id = ?")
            .bind(id)
            .execute(&self.pool))?;
        Ok(())
    }

    // Feishu bindings
    pub async fn create_feishu_binding(
        &self,
        account_id: &str,
        open_id: &str,
        user_id: &str,
    ) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        db_retry!(
            sqlx::query(
                "INSERT INTO feishu_bindings (id, account_id, open_id, user_id, created_at) VALUES (?, ?, ?, ?, ?)
                 ON DUPLICATE KEY UPDATE user_id = VALUES(user_id)"
            )
            .bind(&id)
            .bind(account_id)
            .bind(open_id)
            .bind(user_id)
            .bind(now)
            .execute(&self.pool)
        )?;
        Ok(id)
    }

    pub async fn get_feishu_binding(
        &self,
        account_id: &str,
        open_id: &str,
    ) -> Result<Option<FeishuBinding>> {
        let row = db_retry!(
            sqlx::query_as::<_, FeishuBinding>(
                "SELECT id, account_id, open_id, user_id, created_at FROM feishu_bindings WHERE account_id = ? AND open_id = ?"
            )
            .bind(account_id)
            .bind(open_id)
            .fetch_optional(&self.pool)
        )?;
        Ok(row)
    }

    pub async fn get_feishu_binding_by_id(&self, id: &str) -> Result<Option<FeishuBinding>> {
        let row = db_retry!(sqlx::query_as::<_, FeishuBinding>(
            "SELECT id, account_id, open_id, user_id, created_at FROM feishu_bindings WHERE id = ?"
        )
        .bind(id)
        .fetch_optional(&self.pool))?;
        Ok(row)
    }

    pub async fn get_feishu_binding_by_user(&self, user_id: &str) -> Result<Option<FeishuBinding>> {
        let row = db_retry!(
            sqlx::query_as::<_, FeishuBinding>(
                "SELECT id, account_id, open_id, user_id, created_at FROM feishu_bindings WHERE user_id = ? ORDER BY created_at DESC LIMIT 1"
            )
            .bind(user_id)
            .fetch_optional(&self.pool)
        )?;
        Ok(row)
    }

    pub async fn list_feishu_bindings(&self) -> Result<Vec<FeishuBindingWithUsername>> {
        let rows = db_retry!(
            sqlx::query_as::<_, FeishuBindingWithUsername>(
                "SELECT b.id, b.account_id, b.open_id, b.user_id, u.username, b.created_at FROM feishu_bindings b JOIN users u ON u.id = b.user_id ORDER BY b.created_at DESC"
            )
            .fetch_all(&self.pool)
        )?;
        Ok(rows)
    }

    pub async fn delete_feishu_binding(&self, id: &str) -> Result<()> {
        db_retry!(sqlx::query("DELETE FROM feishu_bindings WHERE id = ?")
            .bind(id)
            .execute(&self.pool))?;
        Ok(())
    }

    // Feishu bind codes
    pub async fn create_feishu_bind_code(
        &self,
        code: &str,
        user_id: &str,
        expires_at: i64,
    ) -> Result<()> {
        db_retry!(sqlx::query(
            "INSERT INTO feishu_bind_codes (code, user_id, expires_at) VALUES (?, ?, ?)"
        )
        .bind(code)
        .bind(user_id)
        .bind(expires_at)
        .execute(&self.pool))?;
        Ok(())
    }

    /// 消费绑定码：存在、未用、未过期则标记已用并返回 user_id
    pub async fn consume_feishu_bind_code(&self, code: &str) -> Result<Option<String>> {
        let now = Utc::now().timestamp_millis();
        let res = db_retry!(
            sqlx::query("UPDATE feishu_bind_codes SET used_at = ? WHERE code = ? AND used_at IS NULL AND expires_at > ?")
                .bind(now)
                .bind(code)
                .bind(now)
                .execute(&self.pool)
        )?;
        if res.rows_affected() == 0 {
            return Ok(None);
        }
        let row: Option<(String,)> = db_retry!(sqlx::query_as(
            "SELECT user_id FROM feishu_bind_codes WHERE code = ?"
        )
        .bind(code)
        .fetch_optional(&self.pool))?;
        Ok(row.map(|r| r.0))
    }

    // Feishu chats（话题=会话映射，账号维度）
    pub async fn get_feishu_chat(
        &self,
        account_id: &str,
        chat_id: &str,
        topic_id: &str,
    ) -> Result<Option<FeishuChat>> {
        let row = db_retry!(
            sqlx::query_as::<_, FeishuChat>(
                "SELECT id, account_id, chat_id, topic_id, session_id, user_id, created_at, updated_at FROM feishu_chats WHERE account_id = ? AND chat_id = ? AND topic_id = ?"
            )
            .bind(account_id)
            .bind(chat_id)
            .bind(topic_id)
            .fetch_optional(&self.pool)
        )?;
        Ok(row)
    }

    /// 按 session 反查飞书话题映射。
    ///
    /// 任务闸门（task_gate）在 cli_agent 内落交互单时只有 session_id，
    /// 需要 account_id / chat_id / topic_id 才能把卡片与 resume 发回原话题。
    /// 同一 session 理论上只对应一条映射；多条时取最近更新的一条。
    pub async fn get_feishu_chat_by_session(&self, session_id: &str) -> Result<Option<FeishuChat>> {
        let row = db_retry!(sqlx::query_as::<_, FeishuChat>(
            "SELECT id, account_id, chat_id, topic_id, session_id, user_id, created_at, updated_at
                 FROM feishu_chats WHERE session_id = ?
                 ORDER BY updated_at DESC
                 LIMIT 1"
        )
        .bind(session_id)
        .fetch_optional(&self.pool))?;
        Ok(row)
    }

    pub async fn set_feishu_chat(
        &self,
        account_id: &str,
        chat_id: &str,
        topic_id: &str,
        session_id: &str,
        user_id: &str,
    ) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        db_retry!(
            sqlx::query(
                "INSERT INTO feishu_chats (id, account_id, chat_id, topic_id, session_id, user_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                 ON DUPLICATE KEY UPDATE session_id = VALUES(session_id), user_id = VALUES(user_id), updated_at = VALUES(updated_at)"
            )
            .bind(&id)
            .bind(account_id)
            .bind(chat_id)
            .bind(topic_id)
            .bind(session_id)
            .bind(user_id)
            .bind(now)
            .bind(now)
            .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn delete_feishu_chat(
        &self,
        account_id: &str,
        chat_id: &str,
        topic_id: &str,
    ) -> Result<()> {
        db_retry!(sqlx::query(
            "DELETE FROM feishu_chats WHERE account_id = ? AND chat_id = ? AND topic_id = ?"
        )
        .bind(account_id)
        .bind(chat_id)
        .bind(topic_id)
        .execute(&self.pool))?;
        Ok(())
    }

    // 飞书 monorepo 部署任务
    #[allow(clippy::too_many_arguments)]
    pub async fn create_deployment(
        &self,
        session_id: &str,
        user_id: &str,
        account_id: &str,
        chat_id: &str,
        topic_id: &str,
        source_dir: &str,
        commit_sha: &str,
        targets: &str,
        summary: &str,
        approval_expires_at: DateTime<Utc>,
    ) -> Result<Deployment> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        db_retry!(sqlx::query(
            "INSERT INTO deployments
                 (id, session_id, user_id, account_id, chat_id, topic_id, source_dir,
                  commit_sha, targets, summary, status, approval_expires_at, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'awaiting_approval', ?, ?, ?)"
        )
        .bind(&id)
        .bind(session_id)
        .bind(user_id)
        .bind(account_id)
        .bind(chat_id)
        .bind(topic_id)
        .bind(source_dir)
        .bind(commit_sha)
        .bind(targets)
        .bind(summary)
        .bind(approval_expires_at)
        .bind(now)
        .bind(now)
        .execute(&self.pool))?;
        self.get_deployment(&id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("deployment inserted but not found: {id}"))
    }

    pub async fn get_deployment(&self, id: &str) -> Result<Option<Deployment>> {
        let row = db_retry!(sqlx::query_as::<_, Deployment>(
            "SELECT id, session_id, user_id, account_id, chat_id, topic_id, source_dir,
                        commit_sha, targets, summary, status, card_message_id, approved_by,
                        approval_expires_at, started_at, finished_at, result, error,
                        created_at, updated_at
                 FROM deployments WHERE id = ?"
        )
        .bind(id)
        .fetch_optional(&self.pool))?;
        Ok(row)
    }

    pub async fn set_deployment_card(&self, id: &str, card_message_id: &str) -> Result<()> {
        db_retry!(sqlx::query(
            "UPDATE deployments SET card_message_id = ?, updated_at = NOW() WHERE id = ?"
        )
        .bind(card_message_id)
        .bind(id)
        .execute(&self.pool))?;
        Ok(())
    }

    /// 原子批准部署：仅任务发起人、未过期且尚待审批时能成功。
    pub async fn approve_deployment(&self, id: &str, user_id: &str) -> Result<Option<Deployment>> {
        let result = db_retry!(sqlx::query(
            "UPDATE deployments
                 SET status = 'approved', approved_by = ?, updated_at = NOW()
                 WHERE id = ? AND user_id = ? AND status = 'awaiting_approval'
                   AND approval_expires_at > NOW()"
        )
        .bind(user_id)
        .bind(id)
        .bind(user_id)
        .execute(&self.pool))?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_deployment(id).await
    }

    pub async fn cancel_deployment(&self, id: &str, user_id: &str) -> Result<bool> {
        let result = db_retry!(sqlx::query(
            "UPDATE deployments
                 SET status = 'cancelled', approved_by = ?, finished_at = NOW(), updated_at = NOW()
                 WHERE id = ? AND user_id = ? AND status = 'awaiting_approval'"
        )
        .bind(user_id)
        .bind(id)
        .bind(user_id)
        .execute(&self.pool))?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn update_deployment_status(
        &self,
        id: &str,
        status: &str,
        result: Option<&str>,
        error: Option<&str>,
    ) -> Result<()> {
        let terminal = matches!(status, "succeeded" | "failed" | "rolled_back" | "cancelled");
        db_retry!(sqlx::query(
            "UPDATE deployments
                 SET status = ?,
                     started_at = CASE WHEN ? IN ('starting', 'building', 'installing')
                                       THEN COALESCE(started_at, NOW()) ELSE started_at END,
                     finished_at = CASE WHEN ? THEN NOW() ELSE finished_at END,
                     result = COALESCE(?, result), error = COALESCE(?, error), updated_at = NOW()
                 WHERE id = ?"
        )
        .bind(status)
        .bind(status)
        .bind(terminal)
        .bind(result)
        .bind(error)
        .bind(id)
        .execute(&self.pool))?;
        Ok(())
    }

    pub async fn list_recoverable_deployments(&self) -> Result<Vec<Deployment>> {
        let rows = db_retry!(
            sqlx::query_as::<_, Deployment>(
                "SELECT id, session_id, user_id, account_id, chat_id, topic_id, source_dir,
                        commit_sha, targets, summary, status, card_message_id, approved_by,
                        approval_expires_at, started_at, finished_at, result, error,
                        created_at, updated_at
                 FROM deployments
                 WHERE status IN ('approved', 'starting', 'building', 'installing', 'restarting', 'verifying')
                 ORDER BY created_at ASC"
            )
            .fetch_all(&self.pool)
        )?;
        Ok(rows)
    }

    /// 最近一次成功发布，用于创建显式回滚审批。
    pub async fn get_latest_successful_deployment(&self) -> Result<Option<Deployment>> {
        let row = db_retry!(sqlx::query_as::<_, Deployment>(
            "SELECT id, session_id, user_id, account_id, chat_id, topic_id, source_dir,
                        commit_sha, targets, summary, status, card_message_id, approved_by,
                        approval_expires_at, started_at, finished_at, result, error,
                        created_at, updated_at
                 FROM deployments
                 WHERE status = 'succeeded'
                 ORDER BY finished_at DESC, created_at DESC
                 LIMIT 1"
        )
        .fetch_optional(&self.pool))?;
        Ok(row)
    }

    // ─── Agent 交互单 ────────────────────────────────────────────────────

    pub async fn create_interaction(&self, input: NewInteraction<'_>) -> Result<AgentInteraction> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        db_retry!(sqlx::query(
            "INSERT INTO agent_interactions
                 (id, session_id, user_id, channel, account_id, chat_id, topic_id,
                  kind, title, goal, analysis, `options`, status, resume_ref, expires_at,
                  created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?, ?, ?)"
        )
        .bind(&id)
        .bind(input.session_id)
        .bind(input.user_id)
        .bind(input.channel)
        .bind(input.account_id)
        .bind(input.chat_id)
        .bind(input.topic_id)
        .bind(input.kind)
        .bind(input.title)
        .bind(input.goal)
        .bind(input.analysis)
        .bind(input.options)
        .bind(input.resume_ref)
        .bind(input.expires_at)
        .bind(now)
        .bind(now)
        .execute(&self.pool))?;
        self.get_interaction(&id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("interaction inserted but not found: {id}"))
    }

    pub async fn get_interaction(&self, id: &str) -> Result<Option<AgentInteraction>> {
        let row = db_retry!(sqlx::query_as::<_, AgentInteraction>(
            "SELECT id, session_id, user_id, channel, account_id,
                        chat_id, topic_id, kind, title, goal, analysis, `options` AS `options`, status,
                        answer, answered_by, answered_at, resume_ref, card_message_id, expires_at,
                        result, error, created_at, updated_at
                 FROM agent_interactions WHERE id = ?"
        )
        .bind(id)
        .fetch_optional(&self.pool))?;
        Ok(row)
    }

    pub async fn set_interaction_card(&self, id: &str, card_message_id: &str) -> Result<()> {
        db_retry!(sqlx::query(
            "UPDATE agent_interactions SET card_message_id = ?, updated_at = NOW() WHERE id = ?"
        )
        .bind(card_message_id)
        .bind(id)
        .execute(&self.pool))?;
        Ok(())
    }

    /// 原子应答：仅 pending 且未过期时成功，防重复点击。返回 None 表示已被抢答或过期。
    ///
    /// 注意 `expires_at IS NULL` 分支——飞书/网页交互单不过期，不能写成
    /// `expires_at > NOW()`，否则永远无法应答。
    pub async fn answer_interaction(
        &self,
        id: &str,
        answer: &str,
        answered_by: &str,
    ) -> Result<Option<AgentInteraction>> {
        let result = db_retry!(sqlx::query(
            "UPDATE agent_interactions
                 SET status = 'answered', answer = ?, answered_by = ?, answered_at = NOW(), updated_at = NOW()
                 WHERE id = ? AND status = 'pending'
                   AND (expires_at IS NULL OR expires_at > NOW())"
        )
        .bind(answer)
        .bind(answered_by)
        .bind(id)
        .execute(&self.pool))?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_interaction(id).await
    }

    /// 派发失败时把交互单退回 pending，让用户可以重新点确认。
    ///
    /// 为什么退回 pending 而不是标 failed：用户的确认意图是真实的，丢掉它等于
    /// 让用户白点一次且无从得知。只回滚 answered——已 done/expired/cancelled 的
    /// 不动，避免覆盖终态。
    pub async fn revert_interaction_to_pending(&self, id: &str) -> Result<bool> {
        let result = db_retry!(sqlx::query(
            "UPDATE agent_interactions
                 SET status = 'pending', answer = NULL, answered_by = NULL,
                     answered_at = NULL, updated_at = NOW()
                 WHERE id = ? AND status = 'answered'"
        )
        .bind(id)
        .execute(&self.pool))?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn update_interaction_status(
        &self,
        id: &str,
        status: &str,
        result: Option<&str>,
        error: Option<&str>,
    ) -> Result<()> {
        db_retry!(sqlx::query(
            "UPDATE agent_interactions
                 SET status = ?,
                     result = COALESCE(?, result),
                     error = COALESCE(?, error),
                     updated_at = NOW()
                 WHERE id = ?"
        )
        .bind(status)
        .bind(result)
        .bind(error)
        .bind(id)
        .execute(&self.pool))?;
        Ok(())
    }

    /// 某会话最近一条可恢复的交互单。
    ///
    /// 含 pending（文字回复路径）与 answered（飞书按钮回调已原子应答、等待 agent
    /// resume）：按钮回调先 answer_interaction 再派发 run_chat_turn，此时状态已是
    /// answered，若只查 pending 会丢单。resume 消费后应变为 done/executing。
    pub async fn latest_pending_interaction(
        &self,
        session_id: &str,
    ) -> Result<Option<AgentInteraction>> {
        let row = db_retry!(sqlx::query_as::<_, AgentInteraction>(
            "SELECT id, session_id, user_id, channel, account_id,
                        chat_id, topic_id, kind, title, goal, analysis, `options` AS `options`, status,
                        answer, answered_by, answered_at, resume_ref, card_message_id, expires_at,
                        result, error, created_at, updated_at
                 FROM agent_interactions
                 WHERE session_id = ? AND status IN ('pending', 'answered')
                 ORDER BY created_at DESC
                 LIMIT 1"
        )
        .bind(session_id)
        .fetch_optional(&self.pool))?;
        Ok(row)
    }

    /// 进程重启收尾：
    /// 1. 已过期的 pending → expired
    /// 2. answered 僵尸 → pending（应答已记录但派发未完成，退回让用户重点）
    /// 3. executing 僵尸 → failed（派发出去了但执行进程已随重启消失）
    ///
    /// 为什么 answered 要退回而不是标失败：用户的确认意图是真实的，
    /// 丢掉它等于让用户白点一次且无从得知。退回 pending 可重试。
    ///
    /// 为什么 executing 要标 failed 而不是退回 pending：那一轮已经真的跑起来过，
    /// 可能已经改了文件，退回 pending 会让用户再点一次从而重复执行。标 failed
    /// 让它离开中间态（否则 admin 永远看到它卡着），用户按需重新派单。
    ///
    /// 僵尸判定都带时间窗，避免误伤「正在派发 / 正在执行」的行。executing 的窗
    /// 取 2 小时，比单轮超时（默认 30 分钟）宽出足够余量。
    /// 返回 `(expired 条数, reverted 条数, failed 条数)`。
    pub async fn expire_stale_interactions(&self) -> Result<(u64, u64, u64)> {
        let expired = db_retry!(sqlx::query(
            "UPDATE agent_interactions
                 SET status = 'expired', updated_at = NOW()
                 WHERE status = 'pending'
                   AND expires_at IS NOT NULL
                   AND expires_at <= NOW()"
        )
        .execute(&self.pool))?;
        let reverted = db_retry!(sqlx::query(
            "UPDATE agent_interactions
                 SET status = 'pending', answer = NULL, answered_by = NULL,
                     answered_at = NULL, updated_at = NOW()
                 WHERE status = 'answered'
                   AND answered_at IS NOT NULL
                   AND answered_at < NOW() - INTERVAL 5 MINUTE"
        )
        .execute(&self.pool))?;
        let failed = db_retry!(sqlx::query(
            "UPDATE agent_interactions
                 SET status = 'failed',
                     error = COALESCE(error, '执行进程已中断（服务重启或异常退出），未收到终态'),
                     updated_at = NOW()
                 WHERE status = 'executing'
                   AND updated_at < NOW() - INTERVAL 2 HOUR"
        )
        .execute(&self.pool))?;
        Ok((
            expired.rows_affected(),
            reverted.rows_affected(),
            failed.rows_affected(),
        ))
    }

    /// 交互单列表（admin）。筛选项均为可选，传 None 表示不限。
    /// 返回 (当页数据, 总条数)，与 list_channel_messages 的既有约定一致。
    ///
    /// 用 `? IS NULL OR col = ?` 做可选筛选，避免字符串拼 SQL；绑定值走 `.bind()`。
    pub async fn list_interactions(
        &self,
        status: Option<&str>,
        kind: Option<&str>,
        channel: Option<&str>,
        page: u32,
        per_page: u32,
    ) -> Result<(Vec<AgentInteraction>, i64)> {
        let (count,): (i64,) = db_retry!(sqlx::query_as(
            "SELECT COUNT(*) FROM agent_interactions
                 WHERE (? IS NULL OR status = ?)
                   AND (? IS NULL OR kind = ?)
                   AND (? IS NULL OR channel = ?)"
        )
        .bind(status)
        .bind(status)
        .bind(kind)
        .bind(kind)
        .bind(channel)
        .bind(channel)
        .fetch_one(&self.pool))?;
        let offset = ((page.saturating_sub(1)) * per_page) as i64;
        let rows = db_retry!(sqlx::query_as::<_, AgentInteraction>(
            "SELECT id, session_id, user_id, channel, account_id,
                        chat_id, topic_id, kind, title, goal, analysis, `options` AS `options`, status,
                        answer, answered_by, answered_at, resume_ref, card_message_id, expires_at,
                        result, error, created_at, updated_at
                 FROM agent_interactions
                 WHERE (? IS NULL OR status = ?)
                   AND (? IS NULL OR kind = ?)
                   AND (? IS NULL OR channel = ?)
                 ORDER BY created_at DESC
                 LIMIT ? OFFSET ?"
        )
        .bind(status)
        .bind(status)
        .bind(kind)
        .bind(kind)
        .bind(channel)
        .bind(channel)
        .bind(per_page as i64)
        .bind(offset)
        .fetch_all(&self.pool))?;
        Ok((rows, count))
    }

    /// 取消待确认交互单。只动 pending，不覆盖终态。
    pub async fn cancel_interaction(&self, id: &str, operator: &str) -> Result<bool> {
        let result = db_retry!(sqlx::query(
            "UPDATE agent_interactions
                 SET status = 'cancelled', answered_by = ?, answered_at = NOW(), updated_at = NOW()
                 WHERE id = ? AND status = 'pending'"
        )
        .bind(operator)
        .bind(id)
        .execute(&self.pool))?;
        Ok(result.rows_affected() == 1)
    }

    /// 作废本会话所有仍 pending 的 task_gate 闸门单，返回受影响行数与它们的卡片 id。
    ///
    /// 为什么需要：用户不点按钮、直接发下一条消息时，那张闸门单会一直 pending 且
    /// 按钮一直可点。等他过很久回头点「开始修」，会在一个已经跑过若干轮、上下文
    /// 完全不同的 thread 上 resume。新一轮开始时就把旧闸门作废，比事后校验便宜。
    pub async fn supersede_pending_task_gates(
        &self,
        session_id: &str,
    ) -> Result<Vec<(String, Option<String>)>> {
        let rows: Vec<(String, Option<String>)> = db_retry!(sqlx::query_as(
            "SELECT id, card_message_id FROM agent_interactions
                 WHERE session_id = ? AND kind = 'task_gate' AND status = 'pending'"
        )
        .bind(session_id)
        .fetch_all(&self.pool))?;
        if rows.is_empty() {
            return Ok(rows);
        }
        db_retry!(sqlx::query(
            "UPDATE agent_interactions
                 SET status = 'cancelled',
                     error = '已被同会话的新一轮任务取代',
                     updated_at = NOW()
                 WHERE session_id = ? AND kind = 'task_gate' AND status = 'pending'"
        )
        .bind(session_id)
        .execute(&self.pool))?;
        Ok(rows)
    }

    // ─── 团队任务流水线（team_tasks / team_task_runs / team_task_events）────────

    /// SELECT 列清单，与 TeamTask / FromRow 字段顺序一致；不用 SELECT *。
    const TEAM_TASK_COLS: &'static str = "id, task_no, session_id, user_id, source, issue_key, \
        title, goal, analysis, status, current_role, dev_rounds, backend, exec_client_id, \
        agent_kind, account_id, chat_id, topic_id, card_message_id, origin_message_id, \
        result, error, created_at, updated_at, finished_at";

    const TEAM_RUN_COLS: &'static str = "id, task_id, role, round, thread_id, status, verdict, \
        handoff, summary, dirty_files, error, started_at, finished_at";

    const TEAM_EVENT_COLS: &'static str =
        "id, task_id, kind, role, round, operator, detail, created_at";

    /// 创建团队任务。task_no 撞唯一键时用新随机种子重试一次；仍失败则返回错误
    /// （连撞两次说明不是巧合，静默重试到成功会掩盖真实问题）。
    pub async fn create_team_task(&self, input: NewTeamTask<'_>) -> Result<TeamTask> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let now_ms = now.timestamp_millis().max(0) as u64;

        // 最多两次：首次 + 唯一键冲突后换种子再试一次
        for attempt in 0u8..2 {
            let seed = Uuid::new_v4().as_u128() as u64;
            let task_no = generate_task_no(now_ms, seed);
            // 不经 db_retry!：需按 Database 错误码区分「task_no 唯一键冲突」与其他错误；
            // db_retry! 会把 sqlx::Error 转成 anyhow，丢失 code() 判定。
            let insert = sqlx::query(
                "INSERT INTO team_tasks
                     (id, task_no, session_id, user_id, source, issue_key, title, goal, analysis,
                      status, current_role, dev_rounds, backend, exec_client_id, agent_kind,
                      account_id, chat_id, topic_id, created_at, updated_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending_confirm', NULL, 0,
                             ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&id)
            .bind(&task_no)
            .bind(input.session_id)
            .bind(input.user_id)
            .bind(input.source)
            .bind(input.issue_key)
            .bind(input.title)
            .bind(input.goal)
            .bind(input.analysis)
            .bind(input.backend)
            .bind(input.exec_client_id)
            .bind(input.agent_kind)
            .bind(input.account_id)
            .bind(input.chat_id)
            .bind(input.topic_id)
            .bind(now)
            .bind(now)
            .execute(&self.pool)
            .await;

            match insert {
                Ok(_) => {
                    return self
                        .get_team_task(&id)
                        .await?
                        .ok_or_else(|| anyhow::anyhow!("team_task inserted but not found: {id}"));
                }
                Err(sqlx::Error::Database(ref e))
                    if e.code().as_deref() == Some("23000") && attempt == 0 =>
                {
                    // task_no 撞键，换种子重试
                    continue;
                }
                Err(e) => return Err(anyhow::Error::from(e)),
            }
        }
        Err(anyhow::anyhow!(
            "create_team_task: task_no unique collision after retry"
        ))
    }

    pub async fn get_team_task(&self, id: &str) -> Result<Option<TeamTask>> {
        let sql = format!(
            "SELECT {} FROM team_tasks WHERE id = ?",
            Self::TEAM_TASK_COLS
        );
        let row = db_retry!(sqlx::query_as::<_, TeamTask>(&sql)
            .bind(id)
            .fetch_optional(&self.pool))?;
        Ok(row)
    }

    /// 按 task_no 查（看板深链用）。
    pub async fn get_team_task_by_no(&self, task_no: &str) -> Result<Option<TeamTask>> {
        let sql = format!(
            "SELECT {} FROM team_tasks WHERE task_no = ?",
            Self::TEAM_TASK_COLS
        );
        let row = db_retry!(sqlx::query_as::<_, TeamTask>(&sql)
            .bind(task_no)
            .fetch_optional(&self.pool))?;
        Ok(row)
    }

    /// 按 session_id 查最新一条未终态任务。编排器从 run 终态反查任务时用。
    pub async fn get_team_task_by_session(&self, session_id: &str) -> Result<Option<TeamTask>> {
        let sql = format!(
            "SELECT {} FROM team_tasks
             WHERE session_id = ? AND status NOT IN ('done','failed','cancelled')
             ORDER BY created_at DESC
             LIMIT 1",
            Self::TEAM_TASK_COLS
        );
        let row = db_retry!(sqlx::query_as::<_, TeamTask>(&sql)
            .bind(session_id)
            .fetch_optional(&self.pool))?;
        Ok(row)
    }

    /// 分页列表。筛选项均为可选，传 None 表示不限。
    /// 用 `? IS NULL OR col = ?` 做可选筛选，避免字符串拼 SQL。
    pub async fn list_team_tasks(
        &self,
        status: Option<&str>,
        user_id: Option<&str>,
        issue_key: Option<&str>,
        page: u32,
        per_page: u32,
    ) -> Result<(Vec<TeamTask>, i64)> {
        let (count,): (i64,) = db_retry!(sqlx::query_as(
            "SELECT COUNT(*) FROM team_tasks
                 WHERE (? IS NULL OR status = ?)
                   AND (? IS NULL OR user_id = ?)
                   AND (? IS NULL OR issue_key = ?)"
        )
        .bind(status)
        .bind(status)
        .bind(user_id)
        .bind(user_id)
        .bind(issue_key)
        .bind(issue_key)
        .fetch_one(&self.pool))?;
        let offset = ((page.saturating_sub(1)) * per_page) as i64;
        let sql = format!(
            "SELECT {} FROM team_tasks
             WHERE (? IS NULL OR status = ?)
               AND (? IS NULL OR user_id = ?)
               AND (? IS NULL OR issue_key = ?)
             ORDER BY created_at DESC
             LIMIT ? OFFSET ?",
            Self::TEAM_TASK_COLS
        );
        let rows = db_retry!(sqlx::query_as::<_, TeamTask>(&sql)
            .bind(status)
            .bind(status)
            .bind(user_id)
            .bind(user_id)
            .bind(issue_key)
            .bind(issue_key)
            .bind(per_page as i64)
            .bind(offset)
            .fetch_all(&self.pool))?;
        Ok((rows, count))
    }

    /// 推进任务状态。current_role 传 None 表示清空（终态）。
    /// 终态（done/failed/cancelled）自动写 finished_at；非终态不动它。
    pub async fn update_team_task_status(
        &self,
        task_id: &str,
        status: &str,
        current_role: Option<&str>,
        result: Option<&str>,
        error: Option<&str>,
    ) -> Result<()> {
        db_retry!(sqlx::query(
            "UPDATE team_tasks
                 SET status = ?,
                     current_role = ?,
                     result = COALESCE(?, result),
                     error = COALESCE(?, error),
                     finished_at = IF(? IN ('done','failed','cancelled'), NOW(), finished_at),
                     updated_at = NOW()
                 WHERE id = ?"
        )
        .bind(status)
        .bind(current_role)
        .bind(result)
        .bind(error)
        .bind(status)
        .bind(task_id)
        .execute(&self.pool))?;
        Ok(())
    }

    /// 写飞书任务主卡 message_id（跨角色原地刷新同一张卡）。
    pub async fn set_team_task_card(&self, task_id: &str, card_message_id: &str) -> Result<()> {
        db_retry!(sqlx::query(
            "UPDATE team_tasks SET card_message_id = ?, updated_at = NOW() WHERE id = ?"
        )
        .bind(card_message_id)
        .bind(task_id)
        .execute(&self.pool))?;
        Ok(())
    }

    /// 回填闸门卡片的 message_id，供后续主卡 reply 使用。
    ///
    /// 主卡要 reply 一条已有消息，而建任务行时还没有卡片
    /// （卡片是 pusher 收到 AskUser 后才发的）。
    pub async fn set_team_task_origin_message(
        &self,
        task_id: &str,
        message_id: &str,
    ) -> Result<()> {
        db_retry!(sqlx::query(
            "UPDATE team_tasks SET origin_message_id = ?, updated_at = NOW() WHERE id = ?"
        )
        .bind(message_id)
        .bind(task_id)
        .execute(&self.pool))?;
        Ok(())
    }

    /// dev_rounds = dev_rounds + 1，返回递增后的值，供 max_dev_rounds 上限判定。
    pub async fn bump_team_task_dev_rounds(&self, task_id: &str) -> Result<i32> {
        db_retry!(
            sqlx::query(
                "UPDATE team_tasks SET dev_rounds = dev_rounds + 1, updated_at = NOW() WHERE id = ?"
            )
            .bind(task_id)
            .execute(&self.pool)
        )?;
        let (rounds,): (i32,) =
            db_retry!(sqlx::query_as("SELECT dev_rounds FROM team_tasks WHERE id = ?")
                .bind(task_id)
                .fetch_one(&self.pool))?;
        Ok(rounds)
    }

    /// 插入角色轮次行。唯一键 (task_id, role, round) 冲突时返回 Ok(None)——
    /// 这是编排器重复派发的正常防线，不是错误，调用方据此跳过派发。
    ///
    /// 错误处理与其它方法不同：不经 `db_retry!`。需要按 `sqlx::Error::Database`
    /// 的 SQLSTATE `23000` 识别唯一键冲突并返回 `Ok(None)`；`db_retry!` 会把错误
    /// 转成 `anyhow::Error`，丢失 `code()` 判定能力。唯一键冲突也不是连接错误，
    /// 不应重试。
    pub async fn insert_team_run(
        &self,
        task_id: &str,
        role: &str,
        round: i32,
    ) -> Result<Option<TeamTaskRun>> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let insert = sqlx::query(
            "INSERT INTO team_task_runs
                 (id, task_id, role, round, status, started_at)
                 VALUES (?, ?, ?, ?, 'running', ?)",
        )
        .bind(&id)
        .bind(task_id)
        .bind(role)
        .bind(round)
        .bind(now)
        .execute(&self.pool)
        .await;

        match insert {
            Ok(_) => {
                let sql = format!(
                    "SELECT {} FROM team_task_runs WHERE id = ?",
                    Self::TEAM_RUN_COLS
                );
                let row = db_retry!(sqlx::query_as::<_, TeamTaskRun>(&sql)
                    .bind(&id)
                    .fetch_optional(&self.pool))?;
                Ok(row)
            }
            Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("23000") => {
                Ok(None)
            }
            Err(e) => Err(anyhow::Error::from(e)),
        }
    }

    /// 写某 run 行的 thread_id（run 的首个事件回来后由编排器调用）。
    pub async fn set_team_run_thread(&self, run_id: &str, thread_id: &str) -> Result<()> {
        db_retry!(
            sqlx::query("UPDATE team_task_runs SET thread_id = ? WHERE id = ?")
                .bind(thread_id)
                .bind(run_id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    /// 角色轮次收尾：写状态、verdict、交接产物、摘要、改动文件数、错误与 finished_at。
    /// status 取 finished / failed / cancelled。
    #[allow(clippy::too_many_arguments)]
    pub async fn finish_team_run(
        &self,
        run_id: &str,
        status: &str,
        verdict: Option<&str>,
        handoff: Option<&str>,
        summary: Option<&str>,
        dirty_files: Option<i32>,
        error: Option<&str>,
    ) -> Result<()> {
        db_retry!(sqlx::query(
            "UPDATE team_task_runs
                 SET status = ?,
                     verdict = ?,
                     handoff = ?,
                     summary = ?,
                     dirty_files = ?,
                     error = ?,
                     finished_at = NOW()
                 WHERE id = ?"
        )
        .bind(status)
        .bind(verdict)
        .bind(handoff)
        .bind(summary)
        .bind(dirty_files)
        .bind(error)
        .bind(run_id)
        .execute(&self.pool))?;
        Ok(())
    }

    /// 按 task_id 查全部轮次，按开始时间升序（看板时间线顺序）。
    pub async fn list_team_runs(&self, task_id: &str) -> Result<Vec<TeamTaskRun>> {
        let sql = format!(
            "SELECT {} FROM team_task_runs WHERE task_id = ? ORDER BY started_at ASC",
            Self::TEAM_RUN_COLS
        );
        let rows = db_retry!(sqlx::query_as::<_, TeamTaskRun>(&sql)
            .bind(task_id)
            .fetch_all(&self.pool))?;
        Ok(rows)
    }

    /// 按 task_id 查最新一行轮次。
    pub async fn latest_team_run(&self, task_id: &str) -> Result<Option<TeamTaskRun>> {
        let sql = format!(
            "SELECT {} FROM team_task_runs WHERE task_id = ? ORDER BY started_at DESC LIMIT 1",
            Self::TEAM_RUN_COLS
        );
        let row = db_retry!(sqlx::query_as::<_, TeamTaskRun>(&sql)
            .bind(task_id)
            .fetch_optional(&self.pool))?;
        Ok(row)
    }

    /// 记一条任务时间线事件。旁路日志语义：写失败不应影响任务本身，
    /// 调用方通常 `let _ =` 或只 warn，不向上传播。
    pub async fn append_team_event(
        &self,
        task_id: &str,
        kind: &str,
        role: Option<&str>,
        round: Option<i32>,
        operator: Option<&str>,
        detail: Option<&str>,
    ) -> Result<()> {
        db_retry!(sqlx::query(
            "INSERT INTO team_task_events (task_id, kind, role, round, operator, detail, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(task_id)
        .bind(kind)
        .bind(role)
        .bind(round)
        .bind(operator)
        .bind(detail)
        .bind(Utc::now())
        .execute(&self.pool))?;
        Ok(())
    }

    /// 读团队任务运行时配置。返回 Ok(None) 表示 DB 里还没有（调用方用 config.toml 兜底）。
    /// JSON 解析失败也返回 Ok(None) 并 warn——配置坏了应该退回默认值继续跑，
    /// 而不是让每个飞书任务都报错。
    pub async fn get_team_task_settings(&self) -> Result<Option<TeamTaskSettings>> {
        let raw = self.get_setting(TEAM_TASK_SETTINGS_KEY).await?;
        let Some(raw) = raw else {
            return Ok(None);
        };
        match serde_json::from_str::<TeamTaskSettings>(&raw) {
            Ok(s) => Ok(Some(s)),
            Err(e) => {
                tracing::warn!(
                    key = TEAM_TASK_SETTINGS_KEY,
                    "team_task_config JSON 解析失败，退回默认值: {e:#}"
                );
                Ok(None)
            }
        }
    }

    /// 覆盖写入团队任务运行时配置。调用方（admin REST）必须先校验。
    pub async fn save_team_task_settings(&self, s: &TeamTaskSettings) -> Result<()> {
        let value = serde_json::to_string(s)?;
        self.set_setting(TEAM_TASK_SETTINGS_KEY, &value).await
    }

    /// 按 task_id 查时间线，按 id 升序（插入顺序即时间顺序）。
    pub async fn list_team_events(&self, task_id: &str) -> Result<Vec<TeamTaskEvent>> {
        let sql = format!(
            "SELECT {} FROM team_task_events WHERE task_id = ? ORDER BY id ASC",
            Self::TEAM_EVENT_COLS
        );
        let rows = db_retry!(sqlx::query_as::<_, TeamTaskEvent>(&sql)
            .bind(task_id)
            .fetch_all(&self.pool))?;
        Ok(rows)
    }

    /// 进程启动时收尾遗留的运行中任务：server 重启后 CLI thread 已不可信，
    /// 一律标失败而不是尝试续跑（仿 scheduler 对遗留 running job_run 的收尾）。
    /// 返回被收尾的任务数。本方法只提供能力，不在 main.rs 调用——接入放在后续任务。
    pub async fn fail_stale_team_tasks(&self) -> Result<u64> {
        // 先收尾这些任务下仍 running 的 run 行，再标任务本身 failed。
        db_retry!(sqlx::query(
            "UPDATE team_task_runs r
                 INNER JOIN team_tasks t ON r.task_id = t.id
                 SET r.status = 'failed',
                     r.finished_at = NOW(),
                     r.error = COALESCE(r.error, 'server 重启，运行中任务已中断')
                 WHERE t.status LIKE 'running_%' AND r.status = 'running'"
        )
        .execute(&self.pool))?;
        let res = db_retry!(sqlx::query(
            "UPDATE team_tasks
                 SET status = 'failed',
                     error = 'server 重启，运行中任务已中断',
                     finished_at = NOW(),
                     current_role = NULL,
                     updated_at = NOW()
                 WHERE status LIKE 'running_%'"
        )
        .execute(&self.pool))?;
        Ok(res.rows_affected())
    }

    // 渠道消息留档
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_channel_message(
        &self,
        channel: &str,
        account_id: &str,
        account_name: &str,
        conversation_id: &str,
        topic_id: &str,
        external_message_id: &str,
        reply_to_external_id: Option<&str>,
        direction: &str,
        message_type: &str,
        content: &str,
        peer_id: Option<&str>,
        user_id: Option<&str>,
        session_id: Option<&str>,
        created_at: DateTime<Utc>,
    ) -> Result<bool> {
        let id = Uuid::new_v4().to_string();
        let result = db_retry!(sqlx::query(
            "INSERT IGNORE INTO channel_messages
                 (id, channel, account_id, account_name, conversation_id, topic_id,
                  external_message_id, reply_to_external_id, direction, message_type,
                  content, peer_id, user_id, username, session_id, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                    (SELECT username FROM users WHERE id = ? LIMIT 1), ?, ?)"
        )
        .bind(&id)
        .bind(channel)
        .bind(account_id)
        .bind(account_name)
        .bind(conversation_id)
        .bind(topic_id)
        .bind(external_message_id)
        .bind(reply_to_external_id)
        .bind(direction)
        .bind(message_type)
        .bind(content)
        .bind(peer_id)
        .bind(user_id)
        .bind(user_id)
        .bind(session_id)
        .bind(created_at)
        .execute(&self.pool))?;
        Ok(result.rows_affected() == 1)
    }

    /// 归档回复消息，并从被回复消息继承聊天、用户和会话上下文。
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_channel_reply(
        &self,
        channel: &str,
        account_id: &str,
        reply_to_external_id: &str,
        external_message_id: &str,
        message_type: &str,
        content: &str,
        created_at: DateTime<Utc>,
    ) -> Result<bool> {
        let id = Uuid::new_v4().to_string();
        let result = db_retry!(sqlx::query(
            "INSERT IGNORE INTO channel_messages
                 (id, channel, account_id, account_name, conversation_id, topic_id,
                  external_message_id, reply_to_external_id, direction, message_type,
                  content, peer_id, user_id, username, session_id, created_at)
                 SELECT ?, channel, account_id, account_name, conversation_id, topic_id,
                    ?, ?, 'outbound', ?, ?, peer_id, user_id, username, session_id, ?
                 FROM channel_messages
                 WHERE channel = ? AND account_id = ? AND external_message_id = ?
                 ORDER BY created_at DESC, id DESC LIMIT 1"
        )
        .bind(&id)
        .bind(external_message_id)
        .bind(reply_to_external_id)
        .bind(message_type)
        .bind(content)
        .bind(created_at)
        .bind(channel)
        .bind(account_id)
        .bind(reply_to_external_id)
        .execute(&self.pool))?;
        Ok(result.rows_affected() == 1)
    }

    /// 主动推送成功后补充其会话上下文；无对应入站消息时仍可作为独立会话展示。
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_channel_outbound(
        &self,
        channel: &str,
        account_id: &str,
        account_name: &str,
        conversation_id: &str,
        topic_id: &str,
        external_message_id: &str,
        message_type: &str,
        content: &str,
        peer_id: Option<&str>,
        user_id: Option<&str>,
        session_id: Option<&str>,
        created_at: DateTime<Utc>,
    ) -> Result<bool> {
        self.insert_channel_message(
            channel,
            account_id,
            account_name,
            conversation_id,
            topic_id,
            external_message_id,
            None,
            "outbound",
            message_type,
            content,
            peer_id,
            user_id,
            session_id,
            created_at,
        )
        .await
    }

    pub async fn update_channel_message_content(
        &self,
        channel: &str,
        account_id: &str,
        external_message_id: &str,
        message_type: &str,
        content: &str,
    ) -> Result<()> {
        db_retry!(sqlx::query(
            "UPDATE channel_messages SET message_type = ?, content = ?
                 WHERE channel = ? AND account_id = ? AND external_message_id = ?"
        )
        .bind(message_type)
        .bind(content)
        .bind(channel)
        .bind(account_id)
        .bind(external_message_id)
        .execute(&self.pool))?;
        Ok(())
    }

    /// 绑定成功后，为绑定消息补上用户快照，确保其回复也能继承用户上下文。
    pub async fn link_channel_message_user(
        &self,
        channel: &str,
        account_id: &str,
        external_message_id: &str,
        user_id: &str,
    ) -> Result<()> {
        db_retry!(sqlx::query(
            "UPDATE channel_messages
                 SET user_id = ?, username = (SELECT username FROM users WHERE id = ? LIMIT 1)
                 WHERE channel = ? AND account_id = ? AND external_message_id = ?"
        )
        .bind(user_id)
        .bind(user_id)
        .bind(channel)
        .bind(account_id)
        .bind(external_message_id)
        .execute(&self.pool))?;
        Ok(())
    }

    /// Agent 会话确定后，为当前渠道消息补上 session 关联。
    pub async fn link_channel_message_session(
        &self,
        channel: &str,
        account_id: &str,
        external_message_id: &str,
        session_id: &str,
        user_id: &str,
    ) -> Result<()> {
        db_retry!(
            sqlx::query(
                "UPDATE channel_messages
                 SET session_id = ?, user_id = COALESCE(user_id, ?),
                     username = COALESCE(username, (SELECT username FROM users WHERE id = ? LIMIT 1))
                 WHERE channel = ? AND account_id = ? AND external_message_id = ?"
            )
            .bind(session_id)
            .bind(user_id)
            .bind(user_id)
            .bind(channel)
            .bind(account_id)
            .bind(external_message_id)
            .execute(&self.pool)
        )?;
        Ok(())
    }

    pub async fn count_channel_conversations(&self, channel: &str, search: &str) -> Result<u64> {
        let pattern = format!("%{search}%");
        let (count,): (i64,) = db_retry!(sqlx::query_as(
            "SELECT COUNT(*) FROM (
                    SELECT account_id, conversation_id, topic_id
                    FROM channel_messages
                    WHERE channel = ? AND (
                        ? = '' OR account_name LIKE ? OR conversation_id LIKE ? OR topic_id LIKE ?
                        OR peer_id LIKE ? OR username LIKE ? OR content LIKE ?
                    )
                    GROUP BY account_id, conversation_id, topic_id
                ) conversations"
        )
        .bind(channel)
        .bind(search)
        .bind(&pattern)
        .bind(&pattern)
        .bind(&pattern)
        .bind(&pattern)
        .bind(&pattern)
        .bind(&pattern)
        .fetch_one(&self.pool))?;
        Ok(count.max(0) as u64)
    }

    pub async fn list_channel_conversations(
        &self,
        channel: &str,
        search: &str,
        page: u32,
        per_page: u32,
    ) -> Result<Vec<ChannelConversation>> {
        let pattern = format!("%{search}%");
        let offset = ((page.saturating_sub(1)) * per_page) as i64;
        let limit = per_page as i64;
        let rows = db_retry!(
            sqlx::query_as::<_, ChannelConversation>(
                "SELECT g.channel, g.account_id,
                    (SELECT m.account_name FROM channel_messages m WHERE m.channel = g.channel AND m.account_id = g.account_id AND m.conversation_id = g.conversation_id AND m.topic_id = g.topic_id ORDER BY m.created_at DESC, m.id DESC LIMIT 1) AS account_name,
                    g.conversation_id, g.topic_id,
                    (SELECT m.peer_id FROM channel_messages m WHERE m.channel = g.channel AND m.account_id = g.account_id AND m.conversation_id = g.conversation_id AND m.topic_id = g.topic_id ORDER BY m.created_at DESC, m.id DESC LIMIT 1) AS peer_id,
                    (SELECT m.user_id FROM channel_messages m WHERE m.channel = g.channel AND m.account_id = g.account_id AND m.conversation_id = g.conversation_id AND m.topic_id = g.topic_id ORDER BY m.created_at DESC, m.id DESC LIMIT 1) AS user_id,
                    (SELECT m.username FROM channel_messages m WHERE m.channel = g.channel AND m.account_id = g.account_id AND m.conversation_id = g.conversation_id AND m.topic_id = g.topic_id ORDER BY m.created_at DESC, m.id DESC LIMIT 1) AS username,
                    (SELECT m.session_id FROM channel_messages m WHERE m.channel = g.channel AND m.account_id = g.account_id AND m.conversation_id = g.conversation_id AND m.topic_id = g.topic_id AND m.session_id IS NOT NULL ORDER BY m.created_at DESC, m.id DESC LIMIT 1) AS session_id,
                    g.message_count, g.first_message_at, g.last_message_at,
                    (SELECT m.direction FROM channel_messages m WHERE m.channel = g.channel AND m.account_id = g.account_id AND m.conversation_id = g.conversation_id AND m.topic_id = g.topic_id ORDER BY m.created_at DESC, m.id DESC LIMIT 1) AS last_direction,
                    (SELECT m.message_type FROM channel_messages m WHERE m.channel = g.channel AND m.account_id = g.account_id AND m.conversation_id = g.conversation_id AND m.topic_id = g.topic_id ORDER BY m.created_at DESC, m.id DESC LIMIT 1) AS last_message_type,
                    (SELECT m.content FROM channel_messages m WHERE m.channel = g.channel AND m.account_id = g.account_id AND m.conversation_id = g.conversation_id AND m.topic_id = g.topic_id ORDER BY m.created_at DESC, m.id DESC LIMIT 1) AS last_content,
                    (SELECT s.provider FROM sessions s WHERE s.id = (SELECT m.session_id FROM channel_messages m WHERE m.channel = g.channel AND m.account_id = g.account_id AND m.conversation_id = g.conversation_id AND m.topic_id = g.topic_id AND m.session_id IS NOT NULL ORDER BY m.created_at DESC, m.id DESC LIMIT 1)) AS agent_provider,
                    (SELECT s.model FROM sessions s WHERE s.id = (SELECT m.session_id FROM channel_messages m WHERE m.channel = g.channel AND m.account_id = g.account_id AND m.conversation_id = g.conversation_id AND m.topic_id = g.topic_id AND m.session_id IS NOT NULL ORDER BY m.created_at DESC, m.id DESC LIMIT 1)) AS agent_model
                 FROM (
                    SELECT channel, account_id, conversation_id, topic_id,
                        COUNT(*) AS message_count, MIN(created_at) AS first_message_at, MAX(created_at) AS last_message_at
                    FROM channel_messages
                    WHERE channel = ? AND (
                        ? = '' OR account_name LIKE ? OR conversation_id LIKE ? OR topic_id LIKE ?
                        OR peer_id LIKE ? OR username LIKE ? OR content LIKE ?
                    )
                    GROUP BY channel, account_id, conversation_id, topic_id
                 ) g
                 ORDER BY g.last_message_at DESC
                 LIMIT ? OFFSET ?"
            )
            .bind(channel)
            .bind(search)
            .bind(&pattern)
            .bind(&pattern)
            .bind(&pattern)
            .bind(&pattern)
            .bind(&pattern)
            .bind(&pattern)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
        )?;
        Ok(rows)
    }

    pub async fn list_channel_messages(
        &self,
        channel: &str,
        account_id: &str,
        conversation_id: &str,
        topic_id: &str,
        page: u32,
        per_page: u32,
    ) -> Result<(Vec<ChannelMessage>, u64)> {
        let (count,): (i64,) = db_retry!(sqlx::query_as(
            "SELECT COUNT(*) FROM channel_messages
                 WHERE channel = ? AND account_id = ? AND conversation_id = ? AND topic_id = ?"
        )
        .bind(channel)
        .bind(account_id)
        .bind(conversation_id)
        .bind(topic_id)
        .fetch_one(&self.pool))?;
        let offset = ((page.saturating_sub(1)) * per_page) as i64;
        let rows = db_retry!(sqlx::query_as::<_, ChannelMessage>(
            "SELECT id, channel, account_id, account_name, conversation_id, topic_id,
                    external_message_id, reply_to_external_id, direction, message_type,
                    content, peer_id, user_id, username, session_id, created_at
                 FROM channel_messages
                 WHERE channel = ? AND account_id = ? AND conversation_id = ? AND topic_id = ?
                 ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?"
        )
        .bind(channel)
        .bind(account_id)
        .bind(conversation_id)
        .bind(topic_id)
        .bind(per_page as i64)
        .bind(offset)
        .fetch_all(&self.pool))?;
        let mut ordered = rows;
        ordered.reverse();
        Ok((ordered, count.max(0) as u64))
    }

    // Job runs（定时任务执行日志）
    /// 写入一条 running 记录，返回行 id
    pub async fn create_job_run(
        &self,
        job_id: &str,
        trigger: &str,
        operator: Option<&str>,
    ) -> Result<i64> {
        let now = Utc::now();
        let res = db_retry!(
            sqlx::query(
                "INSERT INTO job_runs (job_id, `trigger`, status, operator, started_at, created_at) VALUES (?, ?, 'running', ?, ?, ?)"
            )
            .bind(job_id)
            .bind(trigger)
            .bind(operator)
            .bind(now)
            .bind(now)
            .execute(&self.pool)
        )?;
        Ok(res.last_insert_id() as i64)
    }

    pub async fn finish_job_run(
        &self,
        id: i64,
        status: &str,
        result: Option<&str>,
        error: Option<&str>,
    ) -> Result<()> {
        db_retry!(sqlx::query(
            "UPDATE job_runs SET status = ?, finished_at = ?, result = ?, error = ? WHERE id = ?"
        )
        .bind(status)
        .bind(Utc::now())
        .bind(result)
        .bind(error)
        .bind(id)
        .execute(&self.pool))?;
        Ok(())
    }

    /// 启动时收尾：进程崩溃遗留的 running 行标记为 failed
    pub async fn fail_stale_running_job_runs(&self) -> Result<u64> {
        let res = db_retry!(
            sqlx::query("UPDATE job_runs SET status = 'failed', finished_at = ?, error = '进程重启，执行中断' WHERE status = 'running'")
                .bind(Utc::now())
                .execute(&self.pool)
        )?;
        Ok(res.rows_affected())
    }

    pub async fn recent_job_runs(&self, job_id: &str, limit: u32) -> Result<Vec<JobRun>> {
        let rows = db_retry!(
            sqlx::query_as::<_, JobRun>(
                "SELECT id, job_id, `trigger`, status, operator, started_at, finished_at, result, error, created_at FROM job_runs WHERE job_id = ? ORDER BY id DESC LIMIT ?"
            )
            .bind(job_id)
            .bind(limit)
            .fetch_all(&self.pool)
        )?;
        Ok(rows)
    }

    /// 每个 job 每种 trigger 的最新一条（列表页展示用）
    pub async fn latest_job_runs(&self) -> Result<Vec<JobRun>> {
        let rows = db_retry!(
            sqlx::query_as::<_, JobRun>(
                "SELECT r.id, r.job_id, r.`trigger`, r.status, r.operator, r.started_at, r.finished_at, r.result, r.error, r.created_at
                 FROM job_runs r
                 INNER JOIN (SELECT job_id, `trigger`, MAX(id) AS max_id FROM job_runs GROUP BY job_id, `trigger`) latest
                   ON r.id = latest.max_id"
            )
            .fetch_all(&self.pool)
        )?;
        Ok(rows)
    }

    // Job states（启停开关，默认 enabled）
    pub async fn get_job_enabled(&self, job_id: &str) -> Result<bool> {
        let row: Option<(bool,)> = db_retry!(sqlx::query_as(
            "SELECT enabled FROM job_states WHERE job_id = ?"
        )
        .bind(job_id)
        .fetch_optional(&self.pool))?;
        Ok(row.map(|r| r.0).unwrap_or(true))
    }

    pub async fn set_job_enabled(&self, job_id: &str, enabled: bool) -> Result<()> {
        db_retry!(
            sqlx::query(
                "INSERT INTO job_states (job_id, enabled, updated_at) VALUES (?, ?, NOW()) ON DUPLICATE KEY UPDATE enabled = VALUES(enabled), updated_at = NOW()"
            )
            .bind(job_id)
            .bind(enabled)
            .execute(&self.pool)
        )?;
        Ok(())
    }

    /// 所有显式设置过的开关（job_id → enabled）
    pub async fn list_job_states(&self) -> Result<Vec<(String, bool)>> {
        let rows: Vec<(String, bool)> = db_retry!(sqlx::query_as(
            "SELECT job_id, enabled FROM job_states"
        )
        .fetch_all(&self.pool))?;
        Ok(rows)
    }

    // Client agents
    pub async fn upsert_client_agent(
        &self,
        id: &str,
        user_id: &str,
        hostname: Option<&str>,
        work_dir: Option<&str>,
        accept_remote: bool,
    ) -> Result<()> {
        // 注册即一次在线观测（刷 last_seen_at）；UPDATE 故意不碰 enabled，避免重启把停用状态刷回
        db_retry!(
            sqlx::query(
                "INSERT INTO client_agents (id, user_id, hostname, work_dir, accept_remote, last_seen_at) VALUES (?, ?, ?, ?, ?, NOW())
                 ON DUPLICATE KEY UPDATE user_id = VALUES(user_id), hostname = VALUES(hostname), work_dir = VALUES(work_dir), accept_remote = VALUES(accept_remote), last_seen_at = NOW(), updated_at = NOW()"
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

    pub async fn get_client_agent(
        &self,
        user_id: &str,
        client_id: &str,
    ) -> Result<Option<ClientAgent>> {
        let agent = db_retry!(
            sqlx::query_as::<_, ClientAgent>(
                "SELECT id, user_id, hostname, work_dir, accept_remote, enabled, last_active_at, last_seen_at, created_at, updated_at FROM client_agents WHERE user_id = ? AND id = ?"
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
                "SELECT id, user_id, hostname, work_dir, accept_remote, enabled, last_active_at, last_seen_at, created_at, updated_at FROM client_agents WHERE user_id = ? ORDER BY created_at ASC"
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
                "SELECT id, user_id, hostname, work_dir, accept_remote, enabled, last_active_at, last_seen_at, created_at, updated_at FROM client_agents ORDER BY created_at ASC"
            )
            .fetch_all(&self.pool)
        )?;
        Ok(agents)
    }

    /// admin 用：仅按 client_id 查询（dispatch 需要取出 user_id）
    pub async fn get_client_agent_by_id(&self, client_id: &str) -> Result<Option<ClientAgent>> {
        let agent = db_retry!(
            sqlx::query_as::<_, ClientAgent>(
                "SELECT id, user_id, hostname, work_dir, accept_remote, enabled, last_active_at, last_seen_at, created_at, updated_at FROM client_agents WHERE id = ?"
            )
            .bind(client_id)
            .fetch_optional(&self.pool)
        )?;
        Ok(agent)
    }

    /// 停用/启用节点；停用后不再被 pick_online_* 自动选中
    pub async fn set_client_agent_enabled(&self, client_id: &str, enabled: bool) -> Result<()> {
        db_retry!(sqlx::query(
            "UPDATE client_agents SET enabled = ?, updated_at = NOW() WHERE id = ?"
        )
        .bind(enabled)
        .bind(client_id)
        .execute(&self.pool))?;
        Ok(())
    }

    /// 刷新最后运行时间（被派发任务时调用）
    pub async fn touch_client_agent_active(&self, client_id: &str) -> Result<()> {
        db_retry!(
            sqlx::query("UPDATE client_agents SET last_active_at = NOW() WHERE id = ?")
                .bind(client_id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    /// 刷新最后在线时间（poll / 注册时调用；不碰 updated_at）
    pub async fn touch_client_agent_seen(&self, client_id: &str) -> Result<()> {
        db_retry!(
            sqlx::query("UPDATE client_agents SET last_seen_at = NOW() WHERE id = ?")
                .bind(client_id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    /// 上报一条终端通知（kimi task complete / approval 等）
    pub async fn create_client_notification(
        &self,
        user_id: &str,
        client_id: &str,
        term_id: Option<&str>,
        kind: &str,
        title: &str,
        body: Option<&str>,
    ) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        db_retry!(
            sqlx::query(
                "INSERT INTO client_notifications (id, user_id, client_id, term_id, kind, title, body) VALUES (?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(&id)
            .bind(user_id)
            .bind(client_id)
            .bind(term_id)
            .bind(kind)
            .bind(title)
            .bind(body)
            .execute(&self.pool)
        )?;
        Ok(id)
    }

    /// 列出某用户最近的终端通知（后续 admin/微信消费用）
    pub async fn list_client_notifications(
        &self,
        user_id: &str,
        limit: u32,
    ) -> Result<Vec<ClientNotification>> {
        let rows = db_retry!(
            sqlx::query_as::<_, ClientNotification>(
                "SELECT id, user_id, client_id, term_id, kind, title, body, created_at FROM client_notifications WHERE user_id = ? ORDER BY created_at DESC LIMIT ?"
            )
            .bind(user_id)
            .bind(limit)
            .fetch_all(&self.pool)
        )?;
        Ok(rows)
    }

    /// 列出尚未被微信消费方处理的终端通知（全用户，按时间正序）
    pub async fn list_unpushed_client_notifications(
        &self,
        limit: u32,
    ) -> Result<Vec<ClientNotification>> {
        let rows = db_retry!(
            sqlx::query_as::<_, ClientNotification>(
                "SELECT id, user_id, client_id, term_id, kind, title, body, created_at FROM client_notifications WHERE pushed_at IS NULL ORDER BY created_at LIMIT ?"
            )
            .bind(limit)
            .fetch_all(&self.pool)
        )?;
        Ok(rows)
    }

    /// 标记一条终端通知已被微信消费方处理（已推送或不需推送）
    pub async fn mark_client_notification_pushed(&self, id: &str) -> Result<()> {
        db_retry!(
            sqlx::query("UPDATE client_notifications SET pushed_at = NOW() WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    /// 查询 binding 的 kimi 托管会话
    pub async fn get_weixin_kimi(&self, binding_id: &str) -> Result<Option<WeixinKimi>> {
        let row = db_retry!(
            sqlx::query_as::<_, WeixinKimi>(
                "SELECT binding_id, client_id, term_id, work_dir, created_at, updated_at FROM weixin_kimi WHERE binding_id = ?"
            )
            .bind(binding_id)
            .fetch_optional(&self.pool)
        )?;
        Ok(row)
    }

    /// 记录托管终端的工作目录（对下一次 /kimi 生效），保留已有映射
    pub async fn upsert_weixin_kimi_work_dir(
        &self,
        binding_id: &str,
        work_dir: &str,
    ) -> Result<()> {
        db_retry!(
            sqlx::query(
                "INSERT INTO weixin_kimi (binding_id, work_dir, created_at, updated_at) VALUES (?, ?, NOW(), NOW()) ON DUPLICATE KEY UPDATE work_dir = VALUES(work_dir), updated_at = NOW()"
            )
            .bind(binding_id)
            .bind(work_dir)
            .execute(&self.pool)
        )?;
        Ok(())
    }

    /// 建立/更新 binding → 托管终端映射
    pub async fn set_weixin_kimi(
        &self,
        binding_id: &str,
        client_id: &str,
        term_id: &str,
    ) -> Result<()> {
        db_retry!(
            sqlx::query(
                "INSERT INTO weixin_kimi (binding_id, client_id, term_id, created_at, updated_at) VALUES (?, ?, ?, NOW(), NOW()) ON DUPLICATE KEY UPDATE client_id = VALUES(client_id), term_id = VALUES(term_id), updated_at = NOW()"
            )
            .bind(binding_id)
            .bind(client_id)
            .bind(term_id)
            .execute(&self.pool)
        )?;
        Ok(())
    }

    /// 清除托管终端映射（保留 work_dir 供下次 /kimi 使用）
    pub async fn clear_weixin_kimi(&self, binding_id: &str) -> Result<()> {
        db_retry!(
            sqlx::query("UPDATE weixin_kimi SET client_id = NULL, term_id = NULL, updated_at = NOW() WHERE binding_id = ?")
                .bind(binding_id)
                .execute(&self.pool)
        )?;
        Ok(())
    }

    // Switch a session between server-local execution (None) and a remote client
    pub async fn set_session_exec_client(
        &self,
        session_id: &str,
        exec_client_id: Option<&str>,
        work_dir: Option<&str>,
    ) -> Result<()> {
        db_retry!(sqlx::query(
            "UPDATE sessions SET exec_client_id = ?, work_dir = ?, updated_at = NOW() WHERE id = ?"
        )
        .bind(exec_client_id)
        .bind(work_dir)
        .bind(session_id)
        .execute(&self.pool))?;
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

/// 生成人类可读的任务短编号：`tsk_{base36 毫秒时间戳}_{4 位 base36 随机}`。
///
/// 时间戳部分保证大致有序（便于人眼排序与肉眼判断新旧），随机部分避免同毫秒撞车。
/// 真正的唯一性由 team_tasks.uk_team_tasks_no 兜底，调用方撞键时重试一次即可。
pub fn generate_task_no(now_ms: u64, rand_seed: u64) -> String {
    fn base36(mut n: u64, width: usize) -> String {
        const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let mut buf = Vec::new();
        while n > 0 {
            buf.push(DIGITS[(n % 36) as usize]);
            n /= 36;
        }
        while buf.len() < width {
            buf.push(b'0');
        }
        buf.reverse();
        String::from_utf8(buf).expect("base36 digits are ascii")
    }
    format!(
        "tsk_{}_{}",
        base36(now_ms, 1),
        base36(rand_seed % (36 * 36 * 36 * 36), 4)
    )
}

#[cfg(test)]
mod tests {
    use super::generate_task_no;

    #[test]
    fn task_no_format_and_max_len() {
        let no = generate_task_no(1_704_067_200_000, 12345);
        // 形如 tsk_<非空>_<4 位>
        let parts: Vec<&str> = no.split('_').collect();
        assert_eq!(parts.len(), 3, "expected tsk_<ts>_<rand>, got {no}");
        assert_eq!(parts[0], "tsk");
        assert!(!parts[1].is_empty(), "timestamp segment must be non-empty");
        assert_eq!(parts[2].len(), 4, "random segment must be 4 base36 chars");
        assert!(
            no.len() <= 32,
            "task_no must fit VARCHAR(32), got len={} ({no})",
            no.len()
        );
        assert!(
            no.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "task_no should be base36 + underscores: {no}"
        );
    }

    #[test]
    fn task_no_differs_on_rand_seed() {
        let a = generate_task_no(1_000_000, 1);
        let b = generate_task_no(1_000_000, 2);
        assert_ne!(a, b, "same now_ms different rand_seed must differ");
    }

    #[test]
    fn task_no_zero_seed_pads_to_four() {
        let no = generate_task_no(1, 0);
        assert!(
            no.ends_with("_0000"),
            "rand_seed=0 should pad random segment to 0000, got {no}"
        );
    }

    #[test]
    fn task_no_lexicographic_order_with_same_digit_width() {
        // 同一位数宽度下，时间戳递增 → 编号字典序递增（便于人眼排序）
        let a = generate_task_no(36u64.pow(5), 0); // 保证足够大、位数相同的区间
        let b = generate_task_no(36u64.pow(5) + 1, 0);
        let c = generate_task_no(36u64.pow(5) + 100, 0);
        assert!(a < b, "{a} should be < {b}");
        assert!(b < c, "{b} should be < {c}");
    }
}
