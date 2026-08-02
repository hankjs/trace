//! 消息路由：事件解析 → 用户映射 → 斜杠命令 / 任务派发。
//!
//! 复刻 docs/book/agent-os 第 04/06 篇：
//! - text/post 正文提取、@_user_N 提及还原、thread_id/root_id 话题定位
//! - 一个话题 = 一个会话（feishu_chats 映射，topic = thread_id || root_id || "main"）
//! - /new /stop /status /nodes /help 命令管理当前话题会话

use crate::chat::{run_chat_turn, ChatTurnOpts};
use crate::feishu::api::FeishuApi;
use crate::feishu::card::{build_deployment_card, DeploymentCardOptions};
use crate::feishu::pusher;
use crate::provider_registry;
use crate::AppState;
use anyhow::{anyhow, bail, Result};
use base64::Engine as _;
use futures::StreamExt;
use hank_db::FeishuAccount;
use hank_provider::{CompletionRequest, ContentBlock, Message, Role, StreamEvent};
use serde::Deserialize;
use std::sync::Arc;

// ── 事件结构（只取需要的字段，容错未知字段）──

#[derive(Debug, Deserialize)]
struct EventEnvelope {
    #[serde(default)]
    header: EventHeader,
    event: EventData,
}

#[derive(Debug, Default, Deserialize)]
struct EventHeader {
    event_id: Option<String>,
    create_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EventData {
    sender: EventSender,
    message: EventMessage,
}

#[derive(Debug, Deserialize)]
struct EventSender {
    sender_id: Option<SenderId>,
    /// user | app（bot 自己发的消息是 app，必须忽略防止自循环）
    sender_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SenderId {
    open_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EventMessage {
    message_id: String,
    chat_id: String,
    message_type: String,
    /// 双层 JSON：内层需再 parse 一次
    content: String,
    root_id: Option<String>,
    thread_id: Option<String>,
    mentions: Option<Vec<MentionEvent>>,
    create_time: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Mention {
    pub key: String,
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MentionEvent {
    key: String,
    name: Option<String>,
}

/// 收敛后的干净消息（对齐文档 IncomingMessage）
#[derive(Debug)]
pub struct IncomingMessage {
    pub message_id: String,
    pub chat_id: String,
    pub message_type: String,
    pub text: String,
    pub root_id: String,
    pub thread_id: String,
    pub sender_open_id: String,
}

impl IncomingMessage {
    /// 话题定位：thread_id || root_id || "main"（单聊/普通群整聊一个会话）
    pub fn topic_id(&self) -> String {
        if !self.thread_id.is_empty() {
            self.thread_id.clone()
        } else if !self.root_id.is_empty() {
            self.root_id.clone()
        } else {
            "main".to_string()
        }
    }

    /// 是否处于话题内（回复要钉回话题）
    pub fn in_thread(&self) -> bool {
        !self.thread_id.is_empty() || !self.root_id.is_empty()
    }
}

// ── 消息解析（纯函数，可单测）──

/// 从消息 content 提取纯文本（text 直接取；post 遍历富文本段落拼 text/at）。
pub fn extract_text(message_type: &str, content: &str) -> String {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(content) else {
        return String::new();
    };
    match message_type {
        "text" => parsed["text"].as_str().unwrap_or("").to_string(),
        "post" => {
            // post 结构：{"title":..., "content": [[{tag, text?}, ...], ...]}
            // content 也可能是 {"zh_cn": {"content": [[...]]}} 的语种包装
            let paragraphs = if parsed["content"].is_array() {
                parsed["content"].clone()
            } else {
                parsed["zh_cn"]["content"].clone()
            };
            let Some(paragraphs) = paragraphs.as_array() else {
                return String::new();
            };
            let mut out = String::new();
            for para in paragraphs {
                let Some(elements) = para.as_array() else {
                    continue;
                };
                for el in elements {
                    match el["tag"].as_str() {
                        Some("text") => {
                            if let Some(t) = el["text"].as_str() {
                                out.push_str(t);
                            }
                        }
                        Some("at") => {
                            if let Some(name) = el["user_name"].as_str() {
                                out.push_str(&format!("@{name}"));
                            }
                        }
                        _ => {}
                    }
                }
            }
            out.trim().to_string()
        }
        _ => String::new(),
    }
}

/// 把 @_user_N 占位符替换成 @显示名。
pub fn resolve_mentions(text: &str, mentions: &[Mention]) -> String {
    let mut resolved = text.to_string();
    for m in mentions {
        if let Some(name) = &m.name {
            resolved = resolved.replace(&m.key, &format!("@{name}"));
        }
    }
    resolved.trim().to_string()
}

/// 斜杠命令（允许前面带 bot 提及，如 "@MyBot /status"；提及名可含空格，如 "@Agent OS /status"）
#[derive(Debug, PartialEq, Eq)]
pub enum SlashCommand {
    New,
    Stop,
    Status,
    Nodes,
    Diff,
    Test,
    Deploy,
    Rollback,
    Help,
}

/// hank-cli 节点快照（用于 /nodes 回复与 conversation prompt 注入）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HankCliNodeInfo {
    pub client_id: String,
    pub hostname: Option<String>,
    pub online: bool,
    pub work_dir: Option<String>,
    pub agent_backends: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkspaceKind {
    None,
    Repository,
    General,
}

impl WorkspaceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Repository => "repository",
            Self::General => "general",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AgentKind {
    Conversation,
    TraceCode,
    QuantCode,
    GeneralTask,
}

impl AgentKind {
    fn agent_kind(&self) -> &'static str {
        match self {
            Self::Conversation => "conversation",
            Self::TraceCode => "trace_code",
            Self::QuantCode => "quant_code",
            Self::GeneralTask => "general_task",
        }
    }

    fn workspace_kind(&self) -> WorkspaceKind {
        match self {
            Self::Conversation => WorkspaceKind::None,
            Self::TraceCode | Self::QuantCode => WorkspaceKind::Repository,
            Self::GeneralTask => WorkspaceKind::General,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AgentBackend {
    Native,
    Codex,
    Claude,
    Grok,
    Kimi,
}

impl AgentBackend {
    fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Grok => "grok",
            Self::Kimi => "kimi",
        }
    }

    fn preferred(value: &str) -> Self {
        match value {
            "claude" => Self::Claude,
            "grok" => Self::Grok,
            "kimi" => Self::Kimi,
            _ => Self::Codex,
        }
    }
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct NewTopicDecision {
    agent_kind: AgentKind,
    agent_backend: AgentBackend,
}

impl NewTopicDecision {
    fn fallback(default_backend: AgentBackend) -> Self {
        Self {
            agent_kind: AgentKind::GeneralTask,
            agent_backend: default_backend,
        }
    }

    fn agent_kind(&self) -> &'static str {
        self.agent_kind.agent_kind()
    }

    fn workspace_kind(&self) -> WorkspaceKind {
        self.agent_kind.workspace_kind()
    }

    fn normalized(mut self, default_backend: AgentBackend) -> Self {
        match self.agent_kind {
            AgentKind::Conversation => self.agent_backend = AgentBackend::Native,
            _ if self.agent_backend == AgentBackend::Native => self.agent_backend = default_backend,
            _ => {}
        }
        self
    }
}

pub fn parse_command(text: &str) -> Option<SlashCommand> {
    let normalized = text.trim().to_ascii_lowercase();
    let t = normalized.as_str();
    const COMMANDS: [(&str, SlashCommand); 9] = [
        ("/new", SlashCommand::New),
        ("/stop", SlashCommand::Stop),
        ("/status", SlashCommand::Status),
        ("/nodes", SlashCommand::Nodes),
        ("/diff", SlashCommand::Diff),
        ("/test", SlashCommand::Test),
        ("/deploy", SlashCommand::Deploy),
        ("/rollback", SlashCommand::Rollback),
        ("/help", SlashCommand::Help),
    ];
    for (pat, cmd) in COMMANDS {
        if matches_command_text(t, pat) {
            return Some(cmd);
        }
    }
    for alias in ["help", "?help", "? help", "？help", "？ help", "帮助"] {
        if matches_command_text(t, alias) {
            return Some(SlashCommand::Help);
        }
    }
    None
}

fn matches_command_text(text: &str, command: &str) -> bool {
    if text == command {
        return true;
    }
    // "@提及 command" 形式：提及名可包含空格，命令前必须是空白。
    if text.starts_with('@') && text.ends_with(command) {
        let prefix = &text[..text.len() - command.len()];
        return prefix.ends_with(char::is_whitespace);
    }
    false
}

fn parse_bind_code(text: &str) -> Option<&str> {
    let code = text.trim().strip_prefix("bind")?.trim();
    (code.len() == 6 && code.chars().all(|c| c.is_ascii_digit())).then_some(code)
}

pub(crate) fn parse_feishu_timestamp(value: Option<&str>) -> Option<chrono::DateTime<chrono::Utc>> {
    let millis = value?.parse::<i64>().ok()?;
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(millis)
}

// ── 事件入口 ──

pub async fn handle_event(
    state: Arc<AppState>,
    account: FeishuAccount,
    payload: &[u8],
) -> Result<()> {
    // 先只取 header.event_type，避免无关事件被完整反序列化卡住
    let header: serde_json::Value = serde_json::from_slice(payload)?;
    let event_type = header["header"]["event_type"].as_str().unwrap_or("");
    match event_type {
        "im.message.receive_v1" => {
            let envelope: EventEnvelope = serde_json::from_slice(payload)?;
            handle_message(state, account, envelope.header, envelope.event).await
        }
        other => {
            tracing::debug!(event_type = other, "feishu: ignore event");
            Ok(())
        }
    }
}

async fn handle_message(
    state: Arc<AppState>,
    account: FeishuAccount,
    header: EventHeader,
    data: EventData,
) -> Result<()> {
    // 忽略 bot 自己/其他应用的消息，防自循环
    if data.sender.sender_type.as_deref() != Some("user") {
        return Ok(());
    }
    let m = data.message;
    let created_at = parse_feishu_timestamp(m.create_time.as_deref())
        .or_else(|| parse_feishu_timestamp(header.create_time.as_deref()))
        .unwrap_or_else(chrono::Utc::now);
    let mentions: Vec<Mention> = m
        .mentions
        .unwrap_or_default()
        .into_iter()
        .map(|me| Mention {
            key: me.key,
            name: me.name,
        })
        .collect();
    let image_key = extract_image_key(&m.message_type, &m.content);
    let raw_text = extract_text(&m.message_type, &m.content);
    let msg = IncomingMessage {
        message_id: m.message_id,
        chat_id: m.chat_id,
        message_type: m.message_type,
        text: resolve_mentions(&raw_text, &mentions),
        root_id: m.root_id.unwrap_or_default(),
        thread_id: m.thread_id.unwrap_or_default(),
        sender_open_id: data
            .sender
            .sender_id
            .and_then(|s| s.open_id)
            .unwrap_or_default(),
    };

    let log_text = archive_inbound_content(&msg);
    tracing::info!(
        account_id = %account.id,
        event_id = header.event_id.as_deref().unwrap_or(""),
        message_id = %msg.message_id,
        chat = %msg.chat_id,
        topic = %msg.topic_id(),
        sender = %msg.sender_open_id,
        "feishu: 收到消息: {}",
        &log_text.chars().take(80).collect::<String>()
    );

    let api = FeishuApi::new_archived(&account, state.db.clone());

    // 绑定检查：未绑定走 bind code 流程
    let binding = state
        .db
        .get_feishu_binding(&account.id, &msg.sender_open_id)
        .await
        .unwrap_or(None);
    let inserted = state
        .db
        .insert_channel_message(
            "feishu",
            &account.id,
            archive_account_name(&account),
            &msg.chat_id,
            &msg.topic_id(),
            &msg.message_id,
            None,
            "inbound",
            &msg.message_type,
            &archive_inbound_content(&msg),
            Some(&msg.sender_open_id),
            binding.as_ref().map(|binding| binding.user_id.as_str()),
            None,
            created_at,
        )
        .await?;
    if !inserted {
        tracing::info!(message_id = %msg.message_id, "feishu: duplicate inbound message ignored");
        return Ok(());
    }
    let binding = match binding {
        Some(binding) => binding,
        None => {
            handle_unbound(&state, &api, &account, &msg).await;
            return Ok(());
        }
    };
    let user_id = binding.user_id.clone();

    // 不再在消息入口按 server_agent 卡 admin：仅 server worktree/部署路径在
    // create_feishu_session 的 native 分支内做 ensure_server_agent_admin。
    // client-only hank-cli 与纯对话用户不需要 can_login_admin。

    if parse_bind_code(&msg.text).is_some() {
        api.reply_text(
            &msg.message_id,
            "这个飞书账号已经绑定 Trace 用户，直接发送消息即可开始对话",
            msg.in_thread(),
        )
        .await?;
        return Ok(());
    }

    // 斜杠命令
    if let Some(cmd) = parse_command(&msg.text) {
        return handle_command(&state, &api, &account, &msg, &user_id, cmd).await;
    }

    if let Some(image_key) = image_key {
        let bytes = match api
            .download_message_image(&msg.message_id, &image_key)
            .await
        {
            Ok(bytes) => bytes,
            Err(e) => {
                api.reply_text(
                    &msg.message_id,
                    &format!("读取飞书图片失败：{e:#}"),
                    msg.in_thread(),
                )
                .await?;
                return Ok(());
            }
        };
        let Some(media_type) = detect_image_media_type(&bytes) else {
            api.reply_text(&msg.message_id, "暂不支持这种图片格式", msg.in_thread())
                .await?;
            return Ok(());
        };
        let blocks = vec![
            hank_provider::ContentBlock::Text {
                text: "请分析这张飞书图片，并结合当前话题上下文完成任务。".to_string(),
            },
            hank_provider::ContentBlock::Image {
                source: hank_provider::ImageSource {
                    source_type: "base64".to_string(),
                    media_type: media_type.to_string(),
                    data: base64::engine::general_purpose::STANDARD.encode(bytes),
                },
            },
        ];
        return dispatch_task_content(&state, &api, &account, &msg, &user_id, blocks).await;
    }

    if msg.text.is_empty() {
        if msg.message_type != "text" {
            api.reply_text(
                &msg.message_id,
                "收到，但当前只支持文字和图片消息",
                msg.in_thread(),
            )
            .await?;
        }
        return Ok(());
    }

    dispatch_task(&state, &api, &account, &msg, &user_id, &msg.text.clone()).await
}

fn extract_image_key(message_type: &str, content: &str) -> Option<String> {
    if message_type != "image" {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(content).ok()?["image_key"]
        .as_str()
        .filter(|key| !key.is_empty())
        .map(str::to_string)
}

fn detect_image_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// 未绑定用户：bind <6位码> 或提示（与微信同一绑定码模式）。
async fn handle_unbound(
    state: &Arc<AppState>,
    api: &FeishuApi,
    account: &FeishuAccount,
    msg: &IncomingMessage,
) {
    let code = parse_bind_code(&msg.text).map(str::to_string);
    let Some(code) = code else {
        let _ = api
            .reply_text(
                &msg.message_id,
                "你的飞书账号尚未绑定 Trace 用户。\n\n请联系管理员在 Trace 管理后台「飞书机器人 → 用户绑定」生成绑定码；如使用 Trace client，也可在「设置 → 飞书绑定」自行生成。\n\n生成后发送：bind 123456",
                msg.in_thread(),
            )
            .await;
        return;
    };
    match state.db.consume_feishu_bind_code(&code).await {
        Ok(Some(user_id)) => {
            match state
                .db
                .create_feishu_binding(&account.id, &msg.sender_open_id, &user_id)
                .await
            {
                Ok(_) => {
                    tracing::info!(user_id, open_id = %msg.sender_open_id, "feishu binding created");
                    if let Err(e) = state
                        .db
                        .link_channel_message_user("feishu", &account.id, &msg.message_id, &user_id)
                        .await
                    {
                        tracing::warn!(
                            message_id = %msg.message_id,
                            "feishu: link bind message to user failed: {e:#}"
                        );
                    }
                    let _ = api
                        .reply_text(
                            &msg.message_id,
                            "绑定成功！直接发消息即可开始，/help 查看命令",
                            msg.in_thread(),
                        )
                        .await;
                }
                Err(e) => {
                    tracing::warn!("feishu: create binding failed: {e:#}");
                    let _ = api
                        .reply_text(&msg.message_id, "绑定失败，请稍后重试", msg.in_thread())
                        .await;
                }
            }
        }
        Ok(None) => {
            let _ = api
                .reply_text(
                    &msg.message_id,
                    "绑定码无效或已过期。请让管理员在 Trace 管理后台重新生成，或在 Trace client「设置 → 飞书绑定」自行生成。",
                    msg.in_thread(),
                )
                .await;
        }
        Err(e) => {
            tracing::warn!("feishu: consume bind code failed: {e:#}");
        }
    }
}

async fn handle_command(
    state: &Arc<AppState>,
    api: &FeishuApi,
    account: &FeishuAccount,
    msg: &IncomingMessage,
    user_id: &str,
    cmd: SlashCommand,
) -> Result<()> {
    let topic = msg.topic_id();
    match cmd {
        SlashCommand::Help => {
            api.reply_text(
                &msg.message_id,
                "/new 开启新话题\n/stop 停止当前任务\n/status 查看当前会话\n/nodes 列出本机 hank-cli 节点（在线状态与 backends）\n/diff 查看 server worktree 变更（本机 CLI 会话不支持）\n/test 运行 server worktree 测试（本机 CLI 会话不支持）\n/deploy 创建 server 部署审批（本机 CLI 会话不支持）\n/rollback 创建 server 回滚审批（本机 CLI 会话不支持）\n/help 查看命令",
                msg.in_thread(),
            )
            .await?;
        }
        SlashCommand::Nodes => {
            let nodes = collect_hank_cli_nodes(state, user_id).await;
            let text = format_nodes_command_reply(&nodes);
            api.reply_text(&msg.message_id, &text, msg.in_thread())
                .await?;
        }
        SlashCommand::Status => {
            let chat = state
                .db
                .get_feishu_chat(&account.id, &msg.chat_id, &topic)
                .await?;
            let text = match chat {
                Some(c) => {
                    let running = state.active_tasks.read().await.contains_key(&c.session_id)
                        || state.tasks.is_dispatching(&c.session_id).await;
                    let work_dir = state
                        .db
                        .get_session(&c.session_id)
                        .await
                        .ok()
                        .flatten()
                        .and_then(|s| s.work_dir)
                        .unwrap_or_else(|| "未设置".to_string());
                    let mut text = format!(
                        "会话：{}\n状态：{}\n话题：{}\n工作区：{}",
                        c.session_id,
                        if running { "执行中" } else { "空闲" },
                        topic,
                        work_dir
                    );
                    // 执行中就把实时进度一并带上，省得再问一次
                    if running {
                        if let Some(snapshot) = state.tasks.progress(&c.session_id).await {
                            text.push_str(&format!(
                                "\n进度：{}%\n当前：{}\n已用时：{}",
                                snapshot.percent,
                                snapshot.detail,
                                crate::task_state::format_elapsed(snapshot.elapsed())
                            ));
                        }
                    }
                    text
                }
                None => "当前话题还没有会话，直接发消息即可开始".to_string(),
            };
            api.reply_text(&msg.message_id, &text, msg.in_thread())
                .await?;
        }
        SlashCommand::New => {
            // 先停旧任务再删映射（顺序反了会停不到）
            if let Some(old) = state
                .db
                .get_feishu_chat(&account.id, &msg.chat_id, &topic)
                .await?
            {
                if let Some(t) = state.active_tasks.read().await.get(&old.session_id) {
                    t.cancel();
                }
            }
            state
                .db
                .delete_feishu_chat(&account.id, &msg.chat_id, &topic)
                .await?;
            api.reply_text(
                &msg.message_id,
                "已开启新会话，请直接发任务",
                msg.in_thread(),
            )
            .await?;
        }
        SlashCommand::Stop => {
            let chat = state
                .db
                .get_feishu_chat(&account.id, &msg.chat_id, &topic)
                .await?;
            let stopped = match chat {
                Some(c) => state
                    .active_tasks
                    .read()
                    .await
                    .get(&c.session_id)
                    .map(|t| t.cancel())
                    .is_some(),
                None => false,
            };
            let text = if stopped {
                "已停止当前任务"
            } else {
                "当前没有正在执行的任务"
            };
            api.reply_text(&msg.message_id, text, msg.in_thread())
                .await?;
        }
        SlashCommand::Diff => {
            let Some(chat) = state
                .db
                .get_feishu_chat(&account.id, &msg.chat_id, &topic)
                .await?
            else {
                api.reply_text(&msg.message_id, "当前话题还没有代码工作区", msg.in_thread())
                    .await?;
                return Ok(());
            };
            if session_is_client_only(state, &chat.session_id).await? {
                api.reply_text(
                    &msg.message_id,
                    &client_only_command_unsupported_message("/diff"),
                    msg.in_thread(),
                )
                .await?;
                return Ok(());
            }
            let diff =
                match crate::deployment::workspace_diff(state, &chat.session_id, &chat.user_id)
                    .await
                {
                    Ok(diff) => diff,
                    Err(e) => format!("读取变更失败：{e:#}"),
                };
            api.reply_text(&msg.message_id, &diff, msg.in_thread())
                .await?;
        }
        SlashCommand::Test => {
            let Some(chat) = state
                .db
                .get_feishu_chat(&account.id, &msg.chat_id, &topic)
                .await?
            else {
                api.reply_text(&msg.message_id, "当前话题还没有代码工作区", msg.in_thread())
                    .await?;
                return Ok(());
            };
            if session_is_client_only(state, &chat.session_id).await? {
                api.reply_text(
                    &msg.message_id,
                    &client_only_command_unsupported_message("/test"),
                    msg.in_thread(),
                )
                .await?;
                return Ok(());
            }
            if state
                .active_tasks
                .read()
                .await
                .contains_key(&chat.session_id)
            {
                api.reply_text(
                    &msg.message_id,
                    "当前 Agent 仍在执行，请完成或 /stop 后再测试",
                    msg.in_thread(),
                )
                .await?;
                return Ok(());
            }
            api.reply_text(
                &msg.message_id,
                "已开始运行受影响项目的测试",
                msg.in_thread(),
            )
            .await?;
            let state = state.clone();
            let api = api.clone();
            let message_id = msg.message_id.clone();
            let in_thread = msg.in_thread();
            let session_id = chat.session_id.clone();
            let cancel = tokio_util::sync::CancellationToken::new();
            state
                .active_tasks
                .write()
                .await
                .insert(session_id.clone(), cancel.clone());
            tokio::spawn(async move {
                let text = match crate::deployment::test_workspace(
                    &state,
                    &session_id,
                    &chat.user_id,
                    &cancel,
                )
                .await
                {
                    Ok(summary) => format!("测试通过\n{summary}"),
                    Err(e) => format!("测试失败\n{e:#}"),
                };
                state.active_tasks.write().await.remove(&session_id);
                if let Err(e) = api.reply_text(&message_id, &text, in_thread).await {
                    tracing::warn!("feishu: reply test result failed: {e:#}");
                }
            });
        }
        SlashCommand::Deploy => {
            let Some(chat) = state
                .db
                .get_feishu_chat(&account.id, &msg.chat_id, &topic)
                .await?
            else {
                api.reply_text(&msg.message_id, "当前话题还没有代码工作区", msg.in_thread())
                    .await?;
                return Ok(());
            };
            if session_is_client_only(state, &chat.session_id).await? {
                api.reply_text(
                    &msg.message_id,
                    &client_only_command_unsupported_message("/deploy"),
                    msg.in_thread(),
                )
                .await?;
                return Ok(());
            }
            if state
                .active_tasks
                .read()
                .await
                .contains_key(&chat.session_id)
            {
                api.reply_text(
                    &msg.message_id,
                    "当前 Agent 仍在执行，请完成或 /stop 后再部署",
                    msg.in_thread(),
                )
                .await?;
                return Ok(());
            }
            let prepared = match crate::deployment::prepare_deployment(
                state,
                &chat.session_id,
                &chat.user_id,
                &account.id,
                &msg.chat_id,
                &topic,
            )
            .await
            {
                Ok(prepared) => prepared,
                Err(e) => {
                    api.reply_text(
                        &msg.message_id,
                        &format!("无法创建部署：{e:#}"),
                        msg.in_thread(),
                    )
                    .await?;
                    return Ok(());
                }
            };
            let card = build_deployment_card(&DeploymentCardOptions {
                deployment_id: prepared.record.id.clone(),
                session_id: prepared.record.session_id.clone(),
                chat_id: msg.chat_id.clone(),
                topic_id: topic.clone(),
                summary: prepared.record.summary.clone(),
                targets: prepared
                    .targets
                    .iter()
                    .map(|target| target.label().to_string())
                    .collect(),
                diff_stat: prepared.diff_stat,
                expires_at: prepared.record.approval_expires_at.to_rfc3339(),
                approve_label: prepared.approval_label.to_string(),
            });
            let card_message_id = api
                .reply_card(&msg.message_id, &card, msg.in_thread())
                .await?;
            state
                .db
                .set_deployment_card(&prepared.record.id, &card_message_id)
                .await?;
        }
        SlashCommand::Rollback => {
            let Some(chat) = state
                .db
                .get_feishu_chat(&account.id, &msg.chat_id, &topic)
                .await?
            else {
                api.reply_text(&msg.message_id, "当前话题还没有代码工作区", msg.in_thread())
                    .await?;
                return Ok(());
            };
            if session_is_client_only(state, &chat.session_id).await? {
                api.reply_text(
                    &msg.message_id,
                    &client_only_command_unsupported_message("/rollback"),
                    msg.in_thread(),
                )
                .await?;
                return Ok(());
            }
            if state
                .active_tasks
                .read()
                .await
                .contains_key(&chat.session_id)
            {
                api.reply_text(
                    &msg.message_id,
                    "当前 Agent 仍在执行，请完成或 /stop 后再回滚",
                    msg.in_thread(),
                )
                .await?;
                return Ok(());
            }
            let prepared = match crate::deployment::prepare_rollback(
                state,
                &chat.session_id,
                &chat.user_id,
                &account.id,
                &msg.chat_id,
                &topic,
            )
            .await
            {
                Ok(prepared) => prepared,
                Err(e) => {
                    api.reply_text(
                        &msg.message_id,
                        &format!("无法创建回滚：{e:#}"),
                        msg.in_thread(),
                    )
                    .await?;
                    return Ok(());
                }
            };
            let card = build_deployment_card(&DeploymentCardOptions {
                deployment_id: prepared.record.id.clone(),
                session_id: prepared.record.session_id.clone(),
                chat_id: msg.chat_id.clone(),
                topic_id: topic.clone(),
                summary: prepared.record.summary.clone(),
                targets: prepared
                    .targets
                    .iter()
                    .map(|target| target.label().to_string())
                    .collect(),
                diff_stat: prepared.diff_stat,
                expires_at: prepared.record.approval_expires_at.to_rfc3339(),
                approve_label: prepared.approval_label.to_string(),
            });
            let card_message_id = api
                .reply_card(&msg.message_id, &card, msg.in_thread())
                .await?;
            state
                .db
                .set_deployment_card(&prepared.record.id, &card_message_id)
                .await?;
        }
    }
    Ok(())
}

// ── 任务派发 ──

/// 新话题先判断是否真的需要工作区，以及工作区是否属于 Trace monorepo。
/// 分类失败时降级到 general_task + 默认 CLI 后端（client-only），绝不静默建 server worktree。
/// 默认外部 Agent 后端按当前用户在线 hank-cli 节点能力选择，不看 server 侧凭据。
///
/// client-only hank-cli 路由**不依赖** `[server_agent].enabled`：关闭时代码/文件任务
/// 仍走本机节点；开启时仅额外允许 native conversation 使用 server 侧无工具会话。
async fn decide_new_topic(state: &AppState, user_id: &str, text: &str) -> NewTopicDecision {
    let default_backend =
        AgentBackend::preferred(crate::cli_agent::preferred_backend(state, user_id).await);
    let server_agent_enabled = state.config.server_agent.enabled;
    match try_decide_new_topic(state, text, default_backend, server_agent_enabled).await {
        Ok(decision) => {
            let decision = decision.normalized(default_backend);
            tracing::info!(
                ?decision,
                server_agent_enabled,
                "feishu: new topic workspace decision"
            );
            decision
        }
        Err(e) => {
            tracing::warn!("feishu: workspace decision failed, fallback to general: {e:#}");
            NewTopicDecision::fallback(default_backend)
        }
    }
}

async fn try_decide_new_topic(
    state: &AppState,
    text: &str,
    default_backend: AgentBackend,
    server_agent_enabled: bool,
) -> Result<NewTopicDecision> {
    let (record, provider) = provider_registry::resolve_default(&state.db)
        .await
        .ok_or_else(|| anyhow!("没有可用的 LLM provider"))?;
    let env_note = if server_agent_enabled {
        "当前环境已开启 server_agent：conversation 的 native 可在 server 无工具运行；\
         代码/文件任务（trace_code/quant_code/general_task）仍必须走用户本机 hank-cli（codex/claude/grok/kimi），\
         不会在 server bubblewrap 执行。"
            .to_string()
    } else {
        "当前环境未开启 server_agent，没有 server 侧代码工作区（worktree/bubblewrap）。\
         凡需要读改代码、跑命令、操作文件的任务（trace_code / quant_code / general_task）\
         都必须在用户本机在线的 hank-cli 节点上通过 codex/claude/grok/kimi 执行，\
         agent_backend 不可选 native（仅 conversation 可选 native）。\
         没有匹配在线节点时创建会话会明确失败，这是预期行为。"
            .to_string()
    };
    let system = format!("你是飞书任务的路由 Agent。只输出一个 JSON 对象，不要输出 markdown 或其他文字。\n\
        {env_note}\n\
        输出字段 agent_kind 可选值：\n\
        - trace_code：需要读取、修改、测试或部署 Trace/Hank monorepo 的 server、\
          crates、admin、cli、docs、飞书/微信渠道或同步流程；不包括 client 和 quant。\n\
        - quant_code：需要读取、修改或测试 monorepo 的 quant 项目代码、策略、看板或文档。\n\
        - general_task：具体任务与 Trace/quant 无关，但需要文件、代码、命令、下载、分析产物或持续迭代工作区。\n\
        - conversation：用户在问候、讨论、咨询、分析问题，或者尚未给出需要文件和命令的事项。\
          后续对话 Agent 会负责正式回答；路由器不要回答用户问题。\n\
        输出字段 agent_backend 可选值：native、codex、claude、grok、kimi。conversation 必须选 native；\
        其他任务默认选 {default_backend}；用户明确要求 Codex、Claude/Claude Code、Grok 或 Kimi/Kimi Code 时，分别选择对应后端，且 agent_kind 至少为 general_task，不能选 conversation。\n\
        示例：{{\"agent_kind\":\"trace_code\",\"agent_backend\":\"{default_backend}\"}}。\n\
        判断 Agent 必须看语义，不只看是否出现项目名。拿不准是否属于 Trace/quant 时选择 general_task；\
        拿不准是否需要文件或命令时选择 conversation。",
        env_note = env_note,
        default_backend = default_backend.as_str()
    );
    let request = CompletionRequest {
        model: provider_registry::resolve_default_model(&record),
        system: Some(system),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: text.chars().take(4000).collect(),
            }],
        }],
        tools: vec![],
        max_tokens: 320,
    };
    let mut stream = provider.stream(request).await?;
    let mut output = String::new();
    while let Some(event) = stream.next().await {
        match event {
            Ok(StreamEvent::TextDelta(text)) => output.push_str(&text),
            Ok(StreamEvent::MessageEnd { .. }) => break,
            Err(e) => return Err(anyhow!("workspace decision stream failed: {e}")),
            _ => {}
        }
    }
    parse_new_topic_decision(&output)
}

fn parse_new_topic_decision(output: &str) -> Result<NewTopicDecision> {
    let trimmed = output.trim();
    let json = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|value| value.strip_suffix("```"))
        .unwrap_or(trimmed)
        .trim();
    serde_json::from_str(json).map_err(|e| anyhow!("无法解析工作区分类: {e}; output={trimmed}"))
}

fn classification_text(content: &[ContentBlock]) -> String {
    let text = content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    if text.trim().is_empty() {
        "用户发送了一个需要处理的非文本内容。".to_string()
    } else {
        text
    }
}

fn sign_internal_jwt(state: &AppState, user_id: &str, username: &str) -> Result<String> {
    Ok(crate::auth::sign_internal_jwt(
        &state.jwt_secret,
        user_id,
        username,
    )?)
}

/// 派发任务：找/建话题会话，跑一轮 chat，事件流交给 pusher 刷新卡片。
pub async fn dispatch_task(
    state: &Arc<AppState>,
    api: &FeishuApi,
    account: &FeishuAccount,
    msg: &IncomingMessage,
    user_id: &str,
    text: &str,
) -> Result<()> {
    dispatch_task_content(
        state,
        api,
        account,
        msg,
        user_id,
        vec![hank_provider::ContentBlock::Text {
            text: text.to_string(),
        }],
    )
    .await
}

async fn dispatch_task_content(
    state: &Arc<AppState>,
    api: &FeishuApi,
    account: &FeishuAccount,
    msg: &IncomingMessage,
    user_id: &str,
    content: Vec<hank_provider::ContentBlock>,
) -> Result<()> {
    let topic = msg.topic_id();

    // 找/建 feishu_chats 映射的 session。任何初始化错误都要转成用户可见回复，
    // 不能只让 WS 后台任务记一条日志后静默结束。
    let session_result = match state
        .db
        .get_feishu_chat(&account.id, &msg.chat_id, &topic)
        .await
    {
        Ok(Some(c)) => {
            resolve_existing_feishu_session(
                state,
                account,
                msg,
                &topic,
                user_id,
                &content,
                c.session_id,
            )
            .await
        }
        Ok(None) => {
            create_and_map_feishu_session(state, account, msg, &topic, user_id, &content).await
        }
        Err(e) => Err(anyhow!("读取飞书话题会话失败: {e:#}")),
    };
    let session_id = match session_result {
        Ok(Some(session_id)) => session_id,
        Ok(None) => return Ok(()),
        Err(e) => {
            tracing::warn!("feishu: create session workspace failed: {e:#}");
            // 节点缺失、旧 server 会话等错误需要原文回传，不能吞成模糊提示。
            api.reply_text(&msg.message_id, &format!("{e:#}"), msg.in_thread())
                .await?;
            return Ok(());
        }
    };

    if let Err(e) = state
        .db
        .link_channel_message_session("feishu", &account.id, &msg.message_id, &session_id, user_id)
        .await
    {
        tracing::warn!(session_id = %session_id, "feishu: link archived messages to session failed: {e:#}");
    }

    // 并发控制：同 session 同时只跑一个 turn。
    //
    // 只查 active_tasks 不够：run_chat_turn 要先做工作区准备/鉴权/git link 才登记，
    // 这段空窗（秒级）内到达的第二条消息会通过检查、起第二个并发 run（实测表现为
    // 同一话题冒出两张任务卡片）。所以先原子抢派发名额，拿不到就当作"在执行中"。
    let dispatch_guard = state.tasks.try_acquire(&session_id).await;
    let Some(dispatch_guard) = dispatch_guard else {
        api.reply_text(
            &msg.message_id,
            &running_reply(state, &session_id).await,
            msg.in_thread(),
        )
        .await?;
        return Ok(());
    };
    if state.active_tasks.read().await.contains_key(&session_id) {
        dispatch_guard.release().await;
        api.reply_text(
            &msg.message_id,
            &running_reply(state, &session_id).await,
            msg.in_thread(),
        )
        .await?;
        return Ok(());
    }

    // 普通远程工具会话离线后可解除绑定；本机 Agent（client-only）会话在
    // resolve_existing_feishu_session 已强制失败，此处不再解绑或换节点。
    if let Ok(Some(session)) = state.db.get_session(&session_id).await {
        if let Some(ref cid) = session.exec_client_id {
            let local_agent = session
                .metadata
                .as_deref()
                .and_then(parse_session_metadata_json)
                .is_some_and(|metadata| is_client_only_metadata(&metadata));
            if !local_agent && !crate::remote_exec::is_client_online(state, user_id, cid).await {
                tracing::info!(session_id = %session_id, client_id = %cid, "feishu: exec client offline, unbind");
                let _ = state
                    .db
                    .set_session_exec_client(&session_id, None, None)
                    .await;
            }
        }
    }

    let username = state
        .db
        .get_user_by_id(user_id)
        .await
        .ok()
        .flatten()
        .map(|u| u.username)
        .unwrap_or_default();
    let jwt = sign_internal_jwt(state, user_id, &username)?;

    // 仅 conversation（native 无工具）注入链路说明与节点快照，避免模型对 hank-cli 一无所知。
    let extra_prompt_segments = if session_is_conversation(state, &session_id).await {
        build_feishu_conversation_extra_prompts(state, user_id).await
    } else {
        Vec::new()
    };
    let opts = ChatTurnOpts {
        provider: None,
        model: None,
        parent_id: None,
        apply_change_id: None,
        auth_token: jwt,
        extra_prompt_segments,
    };
    let turn = run_chat_turn(state, &session_id, content, opts).await;
    // run_chat_turn 返回时 active_tasks 已登记（或本轮启动失败），
    // 派发名额可以还了，后续并发由 active_tasks 继续挡。
    dispatch_guard.release().await;
    match turn {
        Ok(handle) => {
            pusher::spawn(
                state.clone(),
                api.clone(),
                msg.message_id.clone(),
                msg.chat_id.clone(),
                topic,
                session_id.clone(),
                msg.in_thread(),
                handle.event_rx,
            );
        }
        Err(e) => {
            tracing::warn!("feishu: run_chat_turn failed: {e}");
            state.tasks.clear_progress(&session_id).await;
            api.reply_text(&msg.message_id, &format!("启动失败：{e}"), msg.in_thread())
                .await?;
        }
    }
    Ok(())
}

/// 任务在跑时的回复：带上真实进度，而不是一句静态提示。
///
/// pusher 每次收到事件都会更新 `state.tasks` 里的快照，这里直接读，
/// 所以用户问"进度怎样了 / 怎么动静"能拿到当前百分比、正在做什么、已用多久。
async fn running_reply(state: &Arc<AppState>, session_id: &str) -> String {
    match state.tasks.progress(session_id).await {
        Some(snapshot) => {
            let mut text = format!(
                "任务仍在执行中（{}%）\n当前：{}\n已用时：{}",
                snapshot.percent,
                snapshot.detail,
                crate::task_state::format_elapsed(snapshot.elapsed())
            );
            if snapshot.activities.len() > 1 {
                text.push_str("\n\n最近进展：");
                for activity in &snapshot.activities {
                    text.push_str(&format!("\n· {activity}"));
                }
            }
            text.push_str("\n\n完成后会自动汇报；/stop 可取消");
            text
        }
        // 快照还没建立（刚派发出去、第一个事件未到）
        None => "任务刚开始执行，还没有进度产出；完成后会自动汇报，/stop 可取消".to_string(),
    }
}

async fn resolve_existing_feishu_session(
    state: &Arc<AppState>,
    account: &FeishuAccount,
    msg: &IncomingMessage,
    topic: &str,
    user_id: &str,
    content: &[ContentBlock],
    session_id: String,
) -> Result<Option<String>> {
    let metadata = state
        .db
        .get_session(&session_id)
        .await
        .ok()
        .flatten()
        .and_then(|session| session.metadata);
    let policy = metadata
        .as_deref()
        .map(reuse_policy_for_session_metadata)
        .unwrap_or(SessionReusePolicy::Recreate);

    match policy {
        SessionReusePolicy::ReuseClientOnly { backend, client_id } => {
            // 同一话题固定 backend/client_id；离线或能力不匹配时明确失败，绝不换节点。
            if is_external_agent_backend(&backend) {
                let bound_client_id = match client_id {
                    Some(id) => id,
                    None => state
                        .db
                        .get_session(&session_id)
                        .await
                        .ok()
                        .flatten()
                        .and_then(|session| session.exec_client_id)
                        .ok_or_else(|| {
                            anyhow!(
                                "本机 CLI 会话缺少绑定节点（backend={backend}）。请 /new 后重试。"
                            )
                        })?,
                };
                if !crate::remote_exec::is_client_online(state, user_id, &bound_client_id).await {
                    bail!(
                        "绑定的 hank-cli 节点不在线（client={bound_client_id}, backend={backend}）。请在对应电脑启动 hank-cli 后重试；不会切换到其他节点或 server。"
                    );
                }
                if !crate::remote_exec::client_reports_backend(
                    state,
                    user_id,
                    &bound_client_id,
                    &backend,
                )
                .await
                {
                    bail!(
                        "绑定的 hank-cli 节点未上报 {backend} 能力（client={bound_client_id}）。请检查本机 agent_backends 后重试；不会回退 server 或其他节点。"
                    );
                }
            }
            Ok(Some(session_id))
        }
        SessionReusePolicy::ReuseManaged => Ok(Some(session_id)),
        SessionReusePolicy::RequireNew { backend, .. } => {
            bail!("{}", legacy_server_agent_require_new_message(&backend))
        }
        SessionReusePolicy::Recreate => {
            state
                .db
                .delete_feishu_chat(&account.id, &msg.chat_id, topic)
                .await
                .map_err(|e| anyhow!("重置旧飞书话题会话失败: {e:#}"))?;
            create_and_map_feishu_session(state, account, msg, topic, user_id, content).await
        }
    }
}

async fn session_is_client_only(state: &Arc<AppState>, session_id: &str) -> Result<bool> {
    let metadata = state
        .db
        .get_session(session_id)
        .await?
        .and_then(|session| session.metadata);
    Ok(metadata
        .as_deref()
        .and_then(parse_session_metadata_json)
        .is_some_and(|value| is_client_only_metadata(&value)))
}

async fn create_and_map_feishu_session(
    state: &Arc<AppState>,
    account: &FeishuAccount,
    msg: &IncomingMessage,
    topic: &str,
    user_id: &str,
    content: &[ContentBlock],
) -> Result<Option<String>> {
    // client-only hank-cli 路由始终可用：不因 server_agent.enabled=false 短路成 Native。
    // conversation → native 无工具；代码/文件任务 → 外部 CLI 后端，缺节点时明确失败。
    let decision = decide_new_topic(state, user_id, &classification_text(content)).await;
    let session = create_feishu_session(
        state,
        user_id,
        decision.agent_kind(),
        decision.agent_backend.as_str(),
        decision.workspace_kind(),
    )
    .await?;
    if let Err(e) = state
        .db
        .set_feishu_chat(&account.id, &msg.chat_id, topic, &session.id, user_id)
        .await
    {
        tracing::warn!(session_id = %session.id, "feishu: set chat failed: {e:#}");
    }
    Ok(Some(session.id))
}

fn archive_account_name(account: &FeishuAccount) -> &str {
    if account.name.trim().is_empty() {
        &account.app_id
    } else {
        &account.name
    }
}

fn is_external_agent_backend(backend: &str) -> bool {
    matches!(backend, "codex" | "claude" | "grok" | "kimi")
}

fn parse_session_metadata_json(metadata: &str) -> Option<serde_json::Value> {
    serde_json::from_str(metadata).ok()
}

fn is_client_only_metadata(metadata: &serde_json::Value) -> bool {
    metadata["agent_location"].as_str() == Some("client")
}

/// 历史 server bubblewrap / worktree 会话：`server_agent=true` 且未声明 client-only。
fn is_legacy_server_agent_metadata(metadata: &serde_json::Value) -> bool {
    metadata["server_agent"].as_bool().unwrap_or(false) && !is_client_only_metadata(metadata)
}

/// 外部代码 Agent 会话必须固定在首次绑定的 hank-cli 节点；历史 server-agent 会话不得静默复用。
fn reuse_policy_for_session_metadata(metadata: &str) -> SessionReusePolicy {
    let Some(value) = parse_session_metadata_json(metadata) else {
        return SessionReusePolicy::Recreate;
    };
    let backend = value["agent_backend"].as_str().unwrap_or("native");
    if is_client_only_metadata(&value) {
        return SessionReusePolicy::ReuseClientOnly {
            backend: backend.to_string(),
            client_id: value["exec_client_id"]
                .as_str()
                .map(str::to_string)
                .filter(|id| !id.is_empty()),
        };
    }
    if is_legacy_server_agent_metadata(&value) && is_external_agent_backend(backend) {
        return SessionReusePolicy::RequireNew {
            backend: backend.to_string(),
            reason: "legacy_server_agent",
        };
    }
    if is_legacy_server_agent_metadata(&value) {
        // native conversation 等仍可复用原 server 会话。
        return SessionReusePolicy::ReuseManaged;
    }
    SessionReusePolicy::Recreate
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SessionReusePolicy {
    /// client-only：固定 backend/client_id/thread_id，节点离线时失败而不是换节点。
    ReuseClientOnly {
        backend: String,
        client_id: Option<String>,
    },
    /// 非外部后端的 managed 会话（如 native conversation）。
    ReuseManaged,
    /// 历史 server-agent + 外部后端：禁止静默复用，要求 /new。
    RequireNew {
        backend: String,
        reason: &'static str,
    },
    /// 未管理会话：删除映射后重建。
    Recreate,
}

fn client_only_command_unsupported_message(command: &str) -> String {
    format!(
        "本机 CLI 会话不支持 {command}；该命令面向 server worktree 部署流程。请在本机工作目录自行查看变更、跑测试或部署，或使用 /new 开启新话题。"
    )
}

fn legacy_server_agent_require_new_message(backend: &str) -> String {
    format!(
        "该话题仍绑定已废弃的 server Agent 工作区（backend={backend}），不会再在 wananyun 上执行或修改 Trace。请发送 /new 开启本机 hank-cli 会话，并确保对应电脑在线且已上报 {backend} 能力。"
    )
}

fn missing_agent_node_message(backend: &str) -> String {
    format!(
        "没有在线且支持 {backend} 的 hank-cli 节点。请在目标电脑启动 hank-cli，确认 work_dir 有效且 agent_backends 包含 {backend} 后重试；不会回退到 server bubblewrap 或其他节点。"
    )
}

/// 节点展示名：hostname 优先，否则 client_id 前 8 位。
fn node_display_name(node: &HankCliNodeInfo) -> String {
    match node
        .hostname
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(name) => name.to_string(),
        None => {
            let id = node.client_id.as_str();
            let short = if id.len() > 8 { &id[..8] } else { id };
            short.to_string()
        }
    }
}

/// 渲染 /nodes 命令回复（纯函数，便于单测）。
fn format_nodes_command_reply(nodes: &[HankCliNodeInfo]) -> String {
    if nodes.is_empty() {
        return "当前没有注册的 hank-cli 节点。\n\
请在目标电脑安装并启动 hank-cli，在配置中设置 work_dir 与 agent_backends \
（如 codex/claude/grok/kimi），启动后使用 /nodes 可再次查看。"
            .to_string();
    }
    let mut lines = vec![format!("hank-cli 节点（共 {} 台）：", nodes.len())];
    for node in nodes {
        let status = if node.online { "在线" } else { "离线" };
        let work_dir = node
            .work_dir
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("未设置");
        let backends = if node.agent_backends.is_empty() {
            "（未上报）".to_string()
        } else {
            node.agent_backends.join(", ")
        };
        lines.push(format!(
            "· {} | {} | work_dir={} | backends=[{}]",
            node_display_name(node),
            status,
            work_dir,
            backends
        ));
    }
    lines.join("\n")
}

/// 渲染 conversation 注入用的节点快照（纯函数；数据必须来自调用方真实查询）。
fn render_hank_cli_nodes_snapshot(nodes: &[HankCliNodeInfo]) -> String {
    let online_count = nodes.iter().filter(|n| n.online).count();
    if nodes.is_empty() || online_count == 0 {
        let mut text = "当前没有在线的 hank-cli 节点。".to_string();
        if !nodes.is_empty() {
            text.push_str(" 已注册但全部离线的节点：\n");
            for node in nodes {
                let work_dir = node
                    .work_dir
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("未设置");
                let backends = if node.agent_backends.is_empty() {
                    "（未上报）".to_string()
                } else {
                    node.agent_backends.join(", ")
                };
                text.push_str(&format!(
                    "· {} | 离线 | work_dir={} | backends=[{}]\n",
                    node_display_name(node),
                    work_dir,
                    backends
                ));
            }
        }
        return text.trim_end().to_string();
    }
    let mut lines = vec![format!(
        "当前用户 hank-cli 节点快照（共 {} 台，其中 {} 台在线）：",
        nodes.len(),
        online_count
    )];
    for node in nodes {
        let status = if node.online { "在线" } else { "离线" };
        let work_dir = node
            .work_dir
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("未设置");
        let backends = if node.agent_backends.is_empty() {
            "（未上报）".to_string()
        } else {
            node.agent_backends.join(", ")
        };
        lines.push(format!(
            "· {} | {} | work_dir={} | backends=[{}]",
            node_display_name(node),
            status,
            work_dir,
            backends
        ));
    }
    lines.join("\n")
}

/// 飞书 conversation 会话的链路架构说明（不含节点快照）。
fn feishu_conversation_architecture_text() -> &'static str {
    "【飞书链路与执行架构】\n\
     - 用户通过飞书对话，消息经 Trace server 转发。\n\
     - 代码/文件/命令类任务由 server 路由到用户本机在线的 hank-cli 节点，在节点上调用本机 \
codex / claude（Claude Code）/ grok / kimi CLI 执行，凭据留在本机不上传 server。\n\
     - 一个飞书话题 = 一个会话，话题固定首次选定的 backend 与节点；换后端或换节点需要 /new。\n\
     - 可用命令：/new /stop /status /nodes /help（/diff /test /deploy /rollback 仅 server worktree 会话）。\n\
     - 当前话题是纯对话模式，没有工作目录也没有本地执行工具；用户要跑命令或改代码时应提示用 /new 重新路由。\n\
     以下节点信息来自 server 实时查询，请直接据此回答，不要猜测或编造节点状态。"
}

/// 从 DB + client_hubs 收集用户的 hank-cli 节点快照。
async fn collect_hank_cli_nodes(state: &AppState, user_id: &str) -> Vec<HankCliNodeInfo> {
    let agents = match state.db.list_client_agents(user_id).await {
        Ok(list) => list,
        Err(e) => {
            tracing::warn!(user_id = %user_id, "feishu: list_client_agents failed: {e:#}");
            return Vec::new();
        }
    };
    let mut nodes = Vec::with_capacity(agents.len());
    for agent in agents {
        let online = crate::remote_exec::is_client_online(state, user_id, &agent.id).await;
        // agent_backends 仅在节点 poll/register 时存在于内存 hub；离线时可能为空。
        let agent_backends = {
            let hubs = state.client_hubs.read().await;
            hubs.get(user_id)
                .and_then(|hub| hub.agent_backends.get(&agent.id))
                .cloned()
                .unwrap_or_default()
        };
        nodes.push(HankCliNodeInfo {
            client_id: agent.id,
            hostname: agent.hostname,
            online,
            work_dir: agent.work_dir,
            agent_backends,
        });
    }
    nodes
}

async fn build_feishu_conversation_extra_prompts(state: &AppState, user_id: &str) -> Vec<String> {
    let nodes = collect_hank_cli_nodes(state, user_id).await;
    let snapshot = render_hank_cli_nodes_snapshot(&nodes);
    vec![format!(
        "{}\n\n【当前用户 hank-cli 节点快照】\n{}",
        feishu_conversation_architecture_text(),
        snapshot
    )]
}

async fn session_is_conversation(state: &AppState, session_id: &str) -> bool {
    let metadata = match state.db.get_session(session_id).await {
        Ok(Some(session)) => session.metadata,
        _ => return false,
    };
    metadata
        .as_deref()
        .and_then(parse_session_metadata_json)
        .and_then(|value| value["agent_kind"].as_str().map(|k| k == "conversation"))
        .unwrap_or(false)
}

fn archive_inbound_content(msg: &IncomingMessage) -> String {
    let text = msg.text.trim();
    if text
        .strip_prefix("bind")
        .map(str::trim)
        .is_some_and(|code| code.len() == 6 && code.chars().all(|c| c.is_ascii_digit()))
    {
        return "bind ******".to_string();
    }
    if text.is_empty() {
        format!("[{} message]", msg.message_type)
    } else {
        text.to_string()
    }
}

async fn create_feishu_session(
    state: &Arc<AppState>,
    user_id: &str,
    agent_kind: &str,
    agent_backend: &str,
    workspace_kind: WorkspaceKind,
) -> Result<hank_db::Session> {
    // codex/claude/grok/kimi：必须绑定在线且能力匹配的 hank-cli；绝不回退 server bubblewrap。
    if is_external_agent_backend(agent_backend) {
        let Some(client) =
            crate::remote_exec::pick_online_agent_client(state, user_id, agent_backend).await
        else {
            bail!("{}", missing_agent_node_message(agent_backend));
        };
        let metadata = serde_json::json!({
            "source": "feishu",
            "agent_backend": agent_backend,
            "agent_kind": agent_kind,
            "agent_location": "client",
            "workspace_kind": "client",
            "exec_client_id": client.id,
        })
        .to_string();
        let mut session = state
            .db
            .create_session(
                agent_backend,
                "",
                client.work_dir.as_deref(),
                Some(user_id),
                Some("remote"),
                Some("chat"),
                Some(&metadata),
            )
            .await
            .map_err(|e| anyhow!("create local agent session: {e:#}"))?;
        state
            .db
            .set_session_exec_client(&session.id, Some(&client.id), client.work_dir.as_deref())
            .await?;
        tracing::info!(
            session_id = %session.id,
            client_id = %client.id,
            backend = agent_backend,
            "feishu session bound to hank-cli agent"
        );
        session.exec_client_id = Some(client.id);
        session.work_dir = client.work_dir;
        return Ok(session);
    }

    // native 对话：server-agent 开启时仍可建无 worktree 的 server 会话（不含代码 Agent）。
    // 飞书不再为外部后端创建 server-only / bubblewrap 会话。
    if state.config.server_agent.enabled && agent_backend == "native" {
        crate::deployment::ensure_server_agent_admin(state, user_id).await?;
        let metadata = serde_json::json!({
            "source": "feishu",
            "server_agent": true,
            "agent_backend": agent_backend,
            "agent_kind": agent_kind,
            "workspace_kind": workspace_kind.as_str(),
            "client_excluded": true,
        })
        .to_string();
        let session = state
            .db
            .create_session(
                agent_backend,
                "",
                None,
                Some(user_id),
                Some("server"),
                Some("chat"),
                Some(&metadata),
            )
            .await
            .map_err(|e| anyhow!("create server session: {e:#}"))?;
        // conversation 的 workspace_kind 为 None，不创建目录。
        if workspace_kind != WorkspaceKind::None {
            // 防御：native 非 conversation 理论上已被 normalize 掉；若落到此路径不建 worktree。
            tracing::warn!(
                session_id = %session.id,
                ?workspace_kind,
                "feishu native session unexpectedly requested workspace; skipped"
            );
        }
        return Ok(session);
    }

    let metadata = serde_json::json!({
        "source": "feishu",
        "agent_backend": "native",
        "agent_kind": agent_kind,
    })
    .to_string();
    // conversation 是纯对话：无工具、无工作区，不得绑定 exec_client / work_dir，
    // 否则 chat.rs 会注入「在本地桌面执行」说明，与「没有本地执行工具」自相矛盾。
    // 非 conversation 的 native 会话仍可绑在线桌面 client 使用普通远程工具。
    let client = if should_bind_remote_exec_client(agent_kind) {
        crate::remote_exec::pick_online_client(state, user_id).await
    } else {
        None
    };
    let work_dir = client.as_ref().and_then(|c| c.work_dir.clone());
    let mut session = state
        .db
        .create_session(
            "",
            "",
            work_dir.as_deref(),
            Some(user_id),
            Some("remote"),
            Some("chat"),
            Some(&metadata),
        )
        .await
        .map_err(|e| anyhow!("create session: {e:#}"))?;
    if let Some(c) = client {
        if let Err(e) = state
            .db
            .set_session_exec_client(&session.id, Some(&c.id), c.work_dir.as_deref())
            .await
        {
            tracing::warn!("feishu: bind exec client failed: {e:#}");
        } else {
            tracing::info!(session_id = %session.id, client_id = %c.id, "feishu session bound to desktop client");
            session.exec_client_id = Some(c.id);
            session.work_dir = c.work_dir;
        }
    }
    Ok(session)
}

/// native fallback 分支是否应绑定远程执行节点。
/// conversation 纯对话无工具/无工作区，不得绑定；其余 native 任务可绑桌面 client。
fn should_bind_remote_exec_client(agent_kind: &str) -> bool {
    agent_kind != "conversation"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_text_from_text_message() {
        let content = r#"{"text":"你好世界"}"#;
        assert_eq!(extract_text("text", content), "你好世界");
    }

    #[test]
    fn extract_text_from_post_message() {
        let content = r#"{"title":"","content":[[{"tag":"text","text":"帮我看看 "},{"tag":"at","user_id":"ou_1","user_name":"bot"},{"tag":"text","text":" 这段代码"}]]}"#;
        assert_eq!(extract_text("post", content), "帮我看看 @bot 这段代码");
    }

    #[test]
    fn extract_text_from_post_with_locale_wrapper() {
        let content = r#"{"zh_cn":{"title":"","content":[[{"tag":"text","text":"语种包装"}]]}}"#;
        assert_eq!(extract_text("post", content), "语种包装");
    }

    #[test]
    fn extract_text_unknown_type_is_empty() {
        assert_eq!(extract_text("image", r#"{"image_key":"img_v3_x"}"#), "");
        assert_eq!(extract_text("text", "not json"), "");
    }

    #[test]
    fn parses_and_detects_image_resources() {
        assert_eq!(
            extract_image_key("image", r#"{"image_key":"img_v3_x"}"#).as_deref(),
            Some("img_v3_x")
        );
        assert_eq!(
            detect_image_media_type(b"\x89PNG\r\n\x1a\nrest"),
            Some("image/png")
        );
        assert_eq!(detect_image_media_type(b"not-an-image"), None);
    }

    #[test]
    fn resolve_mentions_replaces_placeholders() {
        let mentions = vec![
            Mention {
                key: "@_user_1".into(),
                name: Some("MyBot".into()),
            },
            Mention {
                key: "@_user_2".into(),
                name: Some("运营专家".into()),
            },
        ];
        assert_eq!(
            resolve_mentions("@_user_1 帮我看看 @_user_2 的代码", &mentions),
            "@MyBot 帮我看看 @运营专家 的代码"
        );
    }

    #[test]
    fn resolve_mentions_skips_nameless() {
        let mentions = vec![Mention {
            key: "@_user_1".into(),
            name: None,
        }];
        assert_eq!(
            resolve_mentions("@_user_1 你好", &mentions),
            "@_user_1 你好"
        );
    }

    #[test]
    fn parse_commands() {
        assert_eq!(parse_command("/new"), Some(SlashCommand::New));
        assert_eq!(parse_command("/stop"), Some(SlashCommand::Stop));
        assert_eq!(parse_command("/status"), Some(SlashCommand::Status));
        assert_eq!(parse_command("/nodes"), Some(SlashCommand::Nodes));
        assert_eq!(parse_command("/diff"), Some(SlashCommand::Diff));
        assert_eq!(parse_command("/test"), Some(SlashCommand::Test));
        assert_eq!(parse_command("/deploy"), Some(SlashCommand::Deploy));
        assert_eq!(parse_command("/rollback"), Some(SlashCommand::Rollback));
        assert_eq!(parse_command("/help"), Some(SlashCommand::Help));
        assert_eq!(parse_command("help"), Some(SlashCommand::Help));
        assert_eq!(parse_command("?help"), Some(SlashCommand::Help));
        assert_eq!(parse_command("？help"), Some(SlashCommand::Help));
        assert_eq!(parse_command("？ help"), Some(SlashCommand::Help));
        assert_eq!(parse_command("帮助"), Some(SlashCommand::Help));
        assert_eq!(parse_command("@Agent OS ？help"), Some(SlashCommand::Help));
        assert_eq!(parse_command("@MyBot /status"), Some(SlashCommand::Status));
        assert_eq!(
            parse_command("@Agent OS /status"),
            Some(SlashCommand::Status)
        );
        assert_eq!(parse_command("@MyBot /nodes"), Some(SlashCommand::Nodes));
        assert_eq!(parse_command("@Agent OS /nodes"), Some(SlashCommand::Nodes));
        assert_eq!(parse_command("帮我运行 /status"), None);
        assert_eq!(parse_command("怎么使用 help"), None);
        assert_eq!(parse_command("/unknown"), None);
        assert_eq!(parse_command("@MyBot"), None);
    }

    #[test]
    fn format_nodes_command_reply_empty() {
        let text = format_nodes_command_reply(&[]);
        assert!(text.contains("没有注册的 hank-cli 节点"), "{text}");
        assert!(text.contains("安装并启动"), "{text}");
        assert!(text.contains("work_dir"), "{text}");
        assert!(text.contains("agent_backends"), "{text}");
    }

    #[test]
    fn render_nodes_snapshot_covers_empty_partial_and_online() {
        // 无节点
        let empty = render_hank_cli_nodes_snapshot(&[]);
        assert!(empty.contains("当前没有在线的 hank-cli 节点"), "{empty}");

        // 全部离线
        let offline_only = vec![HankCliNodeInfo {
            client_id: "abcdef012345".into(),
            hostname: Some("mbp".into()),
            online: false,
            work_dir: Some("/Users/me/proj".into()),
            agent_backends: vec!["codex".into()],
        }];
        let offline_text = render_hank_cli_nodes_snapshot(&offline_only);
        assert!(
            offline_text.contains("当前没有在线的 hank-cli 节点"),
            "{offline_text}"
        );
        assert!(offline_text.contains("mbp"), "{offline_text}");
        assert!(offline_text.contains("离线"), "{offline_text}");
        assert!(offline_text.contains("codex"), "{offline_text}");

        // 部分在线
        let mixed = vec![
            HankCliNodeInfo {
                client_id: "online-1".into(),
                hostname: Some("desk".into()),
                online: true,
                work_dir: Some("/work".into()),
                agent_backends: vec!["claude".into(), "grok".into()],
            },
            HankCliNodeInfo {
                client_id: "offline-1".into(),
                hostname: None,
                online: false,
                work_dir: None,
                agent_backends: vec![],
            },
        ];
        let mixed_text = render_hank_cli_nodes_snapshot(&mixed);
        assert!(mixed_text.contains("desk"), "{mixed_text}");
        assert!(mixed_text.contains("在线"), "{mixed_text}");
        assert!(mixed_text.contains("离线"), "{mixed_text}");
        assert!(mixed_text.contains("claude"), "{mixed_text}");
        // hostname 缺失时用 client_id 前 8 位
        assert!(mixed_text.contains("offline-"), "{mixed_text}");
        assert!(!mixed_text.contains("当前没有在线"), "{mixed_text}");

        let cmd_text = format_nodes_command_reply(&mixed);
        assert!(cmd_text.contains("desk"), "{cmd_text}");
        assert!(cmd_text.contains("在线"), "{cmd_text}");
        assert!(cmd_text.contains("离线"), "{cmd_text}");
    }

    #[test]
    fn architecture_text_mentions_nodes_command_and_cli_chain() {
        let text = feishu_conversation_architecture_text();
        assert!(text.contains("hank-cli"));
        assert!(text.contains("/nodes"));
        assert!(text.contains("codex"));
        assert!(text.contains("/new"));
        assert!(text.contains("纯对话"));
    }

    #[test]
    fn conversation_does_not_bind_remote_exec_client() {
        assert!(!should_bind_remote_exec_client("conversation"));
        assert!(should_bind_remote_exec_client("general_task"));
        assert!(should_bind_remote_exec_client("trace_code"));
        assert!(should_bind_remote_exec_client("quant_code"));
    }

    #[test]
    fn parses_new_topic_workspace_decisions() {
        assert_eq!(
            parse_new_topic_decision(r#"{"agent_kind":"trace_code","agent_backend":"codex"}"#)
                .unwrap(),
            NewTopicDecision {
                agent_kind: AgentKind::TraceCode,
                agent_backend: AgentBackend::Codex,
            }
        );
        assert_eq!(
            parse_new_topic_decision(
                "```json\n{\"agent_kind\":\"quant_code\",\"agent_backend\":\"claude\"}\n```"
            )
            .unwrap(),
            NewTopicDecision {
                agent_kind: AgentKind::QuantCode,
                agent_backend: AgentBackend::Claude,
            }
        );
        assert_eq!(
            parse_new_topic_decision(r#"{"agent_kind":"general_task","agent_backend":"grok"}"#)
                .unwrap()
                .agent_backend,
            AgentBackend::Grok
        );
        assert_eq!(
            parse_new_topic_decision(r#"{"agent_kind":"general_task","agent_backend":"kimi"}"#)
                .unwrap()
                .agent_backend,
            AgentBackend::Kimi
        );
        assert_eq!(
            parse_new_topic_decision(r#"{"agent_kind":"conversation","agent_backend":"native"}"#)
                .unwrap(),
            NewTopicDecision {
                agent_kind: AgentKind::Conversation,
                agent_backend: AgentBackend::Native,
            }
        );
        assert_eq!(
            NewTopicDecision::fallback(AgentBackend::Codex).workspace_kind(),
            WorkspaceKind::General
        );
        assert_eq!(
            AgentKind::QuantCode.workspace_kind(),
            WorkspaceKind::Repository
        );
        assert_eq!(
            NewTopicDecision {
                agent_kind: AgentKind::Conversation,
                agent_backend: AgentBackend::Codex,
            }
            .normalized(AgentBackend::Claude)
            .agent_backend,
            AgentBackend::Native
        );
        assert_eq!(
            NewTopicDecision {
                agent_kind: AgentKind::TraceCode,
                agent_backend: AgentBackend::Native,
            }
            .normalized(AgentBackend::Claude)
            .agent_backend,
            AgentBackend::Claude
        );
    }

    #[test]
    fn client_only_session_reuses_fixed_backend_and_client() {
        let policy = reuse_policy_for_session_metadata(
            r#"{"agent_location":"client","agent_backend":"codex","exec_client_id":"cli-1"}"#,
        );
        assert_eq!(
            policy,
            SessionReusePolicy::ReuseClientOnly {
                backend: "codex".into(),
                client_id: Some("cli-1".into()),
            }
        );
    }

    #[test]
    fn legacy_server_agent_external_session_requires_new() {
        for backend in ["codex", "claude", "grok", "kimi"] {
            let metadata = format!(
                r#"{{"server_agent":true,"agent_backend":"{backend}","workspace_kind":"repository"}}"#
            );
            let policy = reuse_policy_for_session_metadata(&metadata);
            assert_eq!(
                policy,
                SessionReusePolicy::RequireNew {
                    backend: backend.into(),
                    reason: "legacy_server_agent",
                },
                "backend={backend}"
            );
            assert!(legacy_server_agent_require_new_message(backend).contains("/new"));
            assert!(legacy_server_agent_require_new_message(backend).contains(backend));
        }
    }

    #[test]
    fn legacy_server_native_conversation_can_reuse() {
        assert_eq!(
            reuse_policy_for_session_metadata(
                r#"{"server_agent":true,"agent_backend":"native","agent_kind":"conversation"}"#
            ),
            SessionReusePolicy::ReuseManaged
        );
    }

    #[test]
    fn missing_agent_node_message_never_mentions_server_fallback() {
        let text = missing_agent_node_message("claude");
        assert!(text.contains("hank-cli"));
        assert!(text.contains("claude"));
        assert!(text.contains("不会回退"));
        assert!(!text.contains("bubblewrap 继续"));
    }

    #[test]
    fn client_only_commands_are_explicitly_unsupported() {
        for command in ["/diff", "/test", "/deploy", "/rollback"] {
            let text = client_only_command_unsupported_message(command);
            assert!(text.contains(command), "{text}");
            assert!(text.contains("本机 CLI 会话不支持"), "{text}");
            assert!(text.contains("请在本机"), "{text}");
        }
    }

    #[test]
    fn external_backends_are_exactly_four_clis() {
        assert!(is_external_agent_backend("codex"));
        assert!(is_external_agent_backend("claude"));
        assert!(is_external_agent_backend("grok"));
        assert!(is_external_agent_backend("kimi"));
        assert!(!is_external_agent_backend("native"));
        assert!(!is_external_agent_backend("shell"));
    }

    #[test]
    fn classification_text_ignores_image_payload() {
        let content = vec![
            ContentBlock::Text {
                text: "分析这张图".to_string(),
            },
            ContentBlock::Image {
                source: hank_provider::ImageSource {
                    source_type: "base64".to_string(),
                    media_type: "image/png".to_string(),
                    data: "encoded".to_string(),
                },
            },
        ];
        assert_eq!(classification_text(&content), "分析这张图");
        assert_eq!(
            classification_text(&[ContentBlock::Image {
                source: hank_provider::ImageSource {
                    source_type: "base64".to_string(),
                    media_type: "image/png".to_string(),
                    data: "encoded".to_string(),
                },
            }]),
            "用户发送了一个需要处理的非文本内容。"
        );
    }

    #[test]
    fn parses_only_six_digit_bind_codes() {
        assert_eq!(parse_bind_code("bind 181277"), Some("181277"));
        assert_eq!(parse_bind_code("  bind 000001  "), Some("000001"));
        assert_eq!(parse_bind_code("bind 12345"), None);
        assert_eq!(parse_bind_code("bind abcdef"), None);
        assert_eq!(parse_bind_code("binding 123456"), None);
    }

    #[test]
    fn parses_feishu_millisecond_timestamp() {
        let timestamp = parse_feishu_timestamp(Some("1785487725000")).unwrap();
        assert_eq!(timestamp.timestamp_millis(), 1_785_487_725_000);
        assert!(parse_feishu_timestamp(Some("invalid")).is_none());
        assert!(parse_feishu_timestamp(None).is_none());
    }

    #[test]
    fn topic_id_priority() {
        let msg = |thread: &str, root: &str| IncomingMessage {
            message_id: "om_1".into(),
            chat_id: "oc_1".into(),
            message_type: "text".into(),
            text: String::new(),
            root_id: root.into(),
            thread_id: thread.into(),
            sender_open_id: String::new(),
        };
        assert_eq!(msg("omt_1", "").topic_id(), "omt_1");
        assert_eq!(msg("", "om_9").topic_id(), "om_9");
        assert_eq!(msg("", "").topic_id(), "main");
        assert!(msg("omt_1", "").in_thread());
        assert!(msg("", "om_9").in_thread());
        assert!(!msg("", "").in_thread());
    }

    #[test]
    fn archive_content_redacts_bind_code_and_labels_media() {
        let message = |message_type: &str, text: &str| IncomingMessage {
            message_id: "om_1".into(),
            chat_id: "oc_1".into(),
            message_type: message_type.into(),
            text: text.into(),
            root_id: String::new(),
            thread_id: String::new(),
            sender_open_id: "ou_1".into(),
        };
        assert_eq!(
            archive_inbound_content(&message("text", "bind 123456")),
            "bind ******"
        );
        assert_eq!(
            archive_inbound_content(&message("text", "绑定需求")),
            "绑定需求"
        );
        assert_eq!(
            archive_inbound_content(&message("image", "")),
            "[image message]"
        );
    }
}
