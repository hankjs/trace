//! 消息路由：事件解析 → 用户映射 → 斜杠命令 / 任务派发。
//!
//! 复刻 docs/book/agent-os 第 04/06 篇：
//! - text/post 正文提取、@_user_N 提及还原、thread_id/root_id 话题定位
//! - 一个话题 = 一个会话（feishu_chats 映射，topic = thread_id || root_id || "main"）
//! - /new /stop /status /help 命令管理当前话题会话

use crate::chat::{run_chat_turn, ChatTurnOpts};
use crate::feishu::api::FeishuApi;
use crate::feishu::card::{build_deployment_card, DeploymentCardOptions};
use crate::feishu::pusher;
use crate::provider_registry;
use crate::AppState;
use anyhow::{anyhow, Result};
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
                let Some(elements) = para.as_array() else { continue };
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
    Diff,
    Test,
    Deploy,
    Rollback,
    Help,
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
}

impl AgentBackend {
    fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }

    fn preferred(value: &str) -> Self {
        if value == "claude" {
            Self::Claude
        } else {
            Self::Codex
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
            _ if self.agent_backend == AgentBackend::Native => {
                self.agent_backend = default_backend
            }
            _ => {}
        }
        self
    }
}

pub fn parse_command(text: &str) -> Option<SlashCommand> {
    let normalized = text.trim().to_ascii_lowercase();
    let t = normalized.as_str();
    const COMMANDS: [(&str, SlashCommand); 8] = [
        ("/new", SlashCommand::New),
        ("/stop", SlashCommand::Stop),
        ("/status", SlashCommand::Status),
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

pub async fn handle_event(state: Arc<AppState>, account: FeishuAccount, payload: &[u8]) -> Result<()> {
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
        .map(|me| Mention { key: me.key, name: me.name })
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

    if state.config.server_agent.enabled {
        if let Err(e) = crate::deployment::ensure_server_agent_admin(&state, &user_id).await {
            api.reply_text(&msg.message_id, &format!("无权使用 server Agent：{e}"), msg.in_thread())
                .await?;
            return Ok(());
        }
    }

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
        return handle_command(&state, &api, &account, &msg, cmd).await;
    }

    if let Some(image_key) = image_key {
        let bytes = match api.download_message_image(&msg.message_id, &image_key).await {
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
    serde_json::from_str::<serde_json::Value>(content)
        .ok()?["image_key"]
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
    cmd: SlashCommand,
) -> Result<()> {
    let topic = msg.topic_id();
    match cmd {
        SlashCommand::Help => {
            api.reply_text(
                &msg.message_id,
                "/new 开启新话题工作区\n/stop 停止当前任务\n/status 查看当前会话\n/diff 查看代码变更\n/test 运行受影响项目测试\n/deploy 创建部署审批\n/rollback 创建回滚审批\n/help 查看命令",
                msg.in_thread(),
            )
            .await?;
        }
        SlashCommand::Status => {
            let chat = state.db.get_feishu_chat(&account.id, &msg.chat_id, &topic).await?;
            let text = match chat {
                Some(c) => {
                    let running = state.active_tasks.read().await.contains_key(&c.session_id);
                    let work_dir = state
                        .db
                        .get_session(&c.session_id)
                        .await
                        .ok()
                        .flatten()
                        .and_then(|s| s.work_dir)
                        .unwrap_or_else(|| "未设置".to_string());
                    format!(
                        "会话：{}\n状态：{}\n话题：{}\n工作区：{}",
                        c.session_id,
                        if running { "执行中" } else { "空闲" },
                        topic,
                        work_dir
                    )
                }
                None => "当前话题还没有会话，直接发消息即可开始".to_string(),
            };
            api.reply_text(&msg.message_id, &text, msg.in_thread()).await?;
        }
        SlashCommand::New => {
            // 先停旧任务再删映射（顺序反了会停不到）
            if let Some(old) = state.db.get_feishu_chat(&account.id, &msg.chat_id, &topic).await? {
                if let Some(t) = state.active_tasks.read().await.get(&old.session_id) {
                    t.cancel();
                }
            }
            state.db.delete_feishu_chat(&account.id, &msg.chat_id, &topic).await?;
            api.reply_text(&msg.message_id, "已开启新会话，请直接发任务", msg.in_thread())
                .await?;
        }
        SlashCommand::Stop => {
            let chat = state.db.get_feishu_chat(&account.id, &msg.chat_id, &topic).await?;
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
            api.reply_text(&msg.message_id, text, msg.in_thread()).await?;
        }
        SlashCommand::Diff => {
            let Some(chat) = state.db.get_feishu_chat(&account.id, &msg.chat_id, &topic).await? else {
                api.reply_text(&msg.message_id, "当前话题还没有代码工作区", msg.in_thread()).await?;
                return Ok(());
            };
            let diff = match crate::deployment::workspace_diff(state, &chat.session_id, &chat.user_id).await {
                Ok(diff) => diff,
                Err(e) => format!("读取变更失败：{e:#}"),
            };
            api.reply_text(&msg.message_id, &diff, msg.in_thread()).await?;
        }
        SlashCommand::Test => {
            let Some(chat) = state.db.get_feishu_chat(&account.id, &msg.chat_id, &topic).await? else {
                api.reply_text(&msg.message_id, "当前话题还没有代码工作区", msg.in_thread()).await?;
                return Ok(());
            };
            if state.active_tasks.read().await.contains_key(&chat.session_id) {
                api.reply_text(&msg.message_id, "当前 Agent 仍在执行，请完成或 /stop 后再测试", msg.in_thread()).await?;
                return Ok(());
            }
            api.reply_text(&msg.message_id, "已开始运行受影响项目的测试", msg.in_thread()).await?;
            let state = state.clone();
            let api = api.clone();
            let message_id = msg.message_id.clone();
            let in_thread = msg.in_thread();
            let session_id = chat.session_id.clone();
            let cancel = tokio_util::sync::CancellationToken::new();
            state.active_tasks.write().await.insert(session_id.clone(), cancel.clone());
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
            let Some(chat) = state.db.get_feishu_chat(&account.id, &msg.chat_id, &topic).await? else {
                api.reply_text(&msg.message_id, "当前话题还没有代码工作区", msg.in_thread()).await?;
                return Ok(());
            };
            if state.active_tasks.read().await.contains_key(&chat.session_id) {
                api.reply_text(&msg.message_id, "当前 Agent 仍在执行，请完成或 /stop 后再部署", msg.in_thread()).await?;
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
                    api.reply_text(&msg.message_id, &format!("无法创建部署：{e:#}"), msg.in_thread()).await?;
                    return Ok(());
                }
            };
            let card = build_deployment_card(&DeploymentCardOptions {
                deployment_id: prepared.record.id.clone(),
                session_id: prepared.record.session_id.clone(),
                chat_id: msg.chat_id.clone(),
                topic_id: topic.clone(),
                summary: prepared.record.summary.clone(),
                targets: prepared.targets.iter().map(|target| target.label().to_string()).collect(),
                diff_stat: prepared.diff_stat,
                expires_at: prepared.record.approval_expires_at.to_rfc3339(),
                approve_label: prepared.approval_label.to_string(),
            });
            let card_message_id = api.reply_card(&msg.message_id, &card, msg.in_thread()).await?;
            state.db.set_deployment_card(&prepared.record.id, &card_message_id).await?;
        }
        SlashCommand::Rollback => {
            let Some(chat) = state.db.get_feishu_chat(&account.id, &msg.chat_id, &topic).await? else {
                api.reply_text(&msg.message_id, "当前话题还没有代码工作区", msg.in_thread()).await?;
                return Ok(());
            };
            if state.active_tasks.read().await.contains_key(&chat.session_id) {
                api.reply_text(&msg.message_id, "当前 Agent 仍在执行，请完成或 /stop 后再回滚", msg.in_thread()).await?;
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
                    api.reply_text(&msg.message_id, &format!("无法创建回滚：{e:#}"), msg.in_thread()).await?;
                    return Ok(());
                }
            };
            let card = build_deployment_card(&DeploymentCardOptions {
                deployment_id: prepared.record.id.clone(),
                session_id: prepared.record.session_id.clone(),
                chat_id: msg.chat_id.clone(),
                topic_id: topic.clone(),
                summary: prepared.record.summary.clone(),
                targets: prepared.targets.iter().map(|target| target.label().to_string()).collect(),
                diff_stat: prepared.diff_stat,
                expires_at: prepared.record.approval_expires_at.to_rfc3339(),
                approve_label: prepared.approval_label.to_string(),
            });
            let card_message_id = api.reply_card(&msg.message_id, &card, msg.in_thread()).await?;
            state.db.set_deployment_card(&prepared.record.id, &card_message_id).await?;
        }
    }
    Ok(())
}

// ── 任务派发 ──

/// 新话题先判断是否真的需要工作区，以及工作区是否属于 Trace monorepo。
/// 分类失败时降级到普通隔离目录，绝不误创建仓库 worktree。
async fn decide_new_topic(state: &AppState, text: &str) -> NewTopicDecision {
    let default_backend =
        AgentBackend::preferred(crate::cli_agent::preferred_backend(state).await);
    match try_decide_new_topic(state, text, default_backend).await {
        Ok(decision) => {
            let decision = decision.normalized(default_backend);
            tracing::info!(?decision, "feishu: new topic workspace decision");
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
) -> Result<NewTopicDecision> {
    let (record, provider) = provider_registry::resolve_default(&state.db)
        .await
        .ok_or_else(|| anyhow!("没有可用的 LLM provider"))?;
    let system = format!("你是飞书任务的路由 Agent。只输出一个 JSON 对象，不要输出 markdown 或其他文字。\n\
        输出字段 agent_kind 可选值：\n\
        - trace_code：需要读取、修改、测试或部署 Trace/Hank monorepo 的 server、\
          crates、admin、cli、docs、飞书/微信渠道或同步流程；不包括 client 和 quant。\n\
        - quant_code：需要读取、修改或测试 monorepo 的 quant 项目代码、策略、看板或文档。\n\
        - general_task：具体任务与 Trace/quant 无关，但需要文件、代码、命令、下载、分析产物或持续迭代工作区。\n\
        - conversation：用户在问候、讨论、咨询、分析问题，或者尚未给出需要文件和命令的事项。\
          后续对话 Agent 会负责正式回答；路由器不要回答用户问题。\n\
        输出字段 agent_backend 可选值：native、codex、claude。conversation 必须选 native；\
        其他任务默认选 {default_backend}；用户明确要求 Codex 时选 codex，明确要求 Claude/Claude Code，或任务明确是 Claude Code 配置与插件维护时选 claude。\n\
        示例：{{\"agent_kind\":\"trace_code\",\"agent_backend\":\"{default_backend}\"}}。\n\
        判断 Agent 必须看语义，不只看是否出现项目名。拿不准是否属于 Trace/quant 时选择 general_task；\
        拿不准是否需要文件或命令时选择 conversation。",
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
    serde_json::from_str(json)
        .map_err(|e| anyhow!("无法解析工作区分类: {e}; output={trimmed}"))
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
    Ok(crate::auth::sign_internal_jwt(&state.jwt_secret, user_id, username)?)
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
    let session_result = match state.db.get_feishu_chat(&account.id, &msg.chat_id, &topic).await {
        Ok(Some(c)) if state.config.server_agent.enabled => {
            let is_server_session = state
                .db
                .get_session(&c.session_id)
                .await
                .ok()
                .flatten()
                .and_then(|session| session.metadata)
                .and_then(|metadata| serde_json::from_str::<serde_json::Value>(&metadata).ok())
                .and_then(|metadata| metadata["server_agent"].as_bool())
                .unwrap_or(false);
            if is_server_session {
                Ok(Some(c.session_id))
            } else {
                match state.db.delete_feishu_chat(&account.id, &msg.chat_id, &topic).await {
                    Ok(()) => create_and_map_feishu_session(
                        state, account, msg, &topic, user_id, &content,
                    )
                    .await,
                    Err(e) => Err(anyhow!("重置旧飞书话题会话失败: {e:#}")),
                }
            }
        }
        Ok(Some(c)) => Ok(Some(c.session_id)),
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
            api.reply_text(
                &msg.message_id,
                "创建工作区失败，已记录日志，请稍后重试",
                msg.in_thread(),
            )
            .await?;
            return Ok(());
        }
    };

    if let Err(e) = state
        .db
        .link_channel_message_session(
            "feishu",
            &account.id,
            &msg.message_id,
            &session_id,
            user_id,
        )
        .await
    {
        tracing::warn!(session_id = %session_id, "feishu: link archived messages to session failed: {e:#}");
    }

    // 并发控制：同 session 同时只跑一个 turn
    if state.active_tasks.read().await.contains_key(&session_id) {
        api.reply_text(&msg.message_id, "正在执行中，/stop 可取消", msg.in_thread())
            .await?;
        return Ok(());
    }

    // 会话绑定的桌面 client 已离线时解除绑定，退化为 server 本地执行
    if let Ok(Some(session)) = state.db.get_session(&session_id).await {
        if let Some(ref cid) = session.exec_client_id {
            if !crate::remote_exec::is_client_online(state, user_id, cid).await {
                tracing::info!(session_id = %session_id, client_id = %cid, "feishu: exec client offline, unbind");
                let _ = state.db.set_session_exec_client(&session_id, None, None).await;
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

    let opts = ChatTurnOpts {
        provider: None,
        model: None,
        parent_id: None,
        apply_change_id: None,
        auth_token: jwt,
    };
    match run_chat_turn(state, &session_id, content, opts).await {
        Ok(handle) => {
            pusher::spawn(
                state.clone(),
                api.clone(),
                msg.message_id.clone(),
                msg.chat_id.clone(),
                topic,
                session_id,
                msg.in_thread(),
                handle.event_rx,
            );
        }
        Err(e) => {
            tracing::warn!("feishu: run_chat_turn failed: {e}");
            api.reply_text(&msg.message_id, &format!("启动失败：{e}"), msg.in_thread())
                .await?;
        }
    }
    Ok(())
}

async fn create_and_map_feishu_session(
    state: &Arc<AppState>,
    account: &FeishuAccount,
    msg: &IncomingMessage,
    topic: &str,
    user_id: &str,
    content: &[ContentBlock],
) -> Result<Option<String>> {
    let decision = if state.config.server_agent.enabled {
        decide_new_topic(state, &classification_text(content)).await
    } else {
        NewTopicDecision::fallback(AgentBackend::Codex)
    };
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
    if state.config.server_agent.enabled {
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
        let mut session = state
            .db
            .create_session(
                // provider 记录实际执行后端（codex / claude / native），admin 列表据此区分；
                // model 建会话时还未确定，由 cli_agent 首轮解析出真实模型名后回写。
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
        let workspace = match workspace_kind {
            WorkspaceKind::None => Ok(None),
            WorkspaceKind::Repository => {
                crate::deployment::prepare_repository_workspace(state, &session.id)
                    .await
                    .map(Some)
            }
            WorkspaceKind::General => {
                crate::deployment::prepare_general_workspace(state, &session.id)
                    .await
                    .map(Some)
            }
        };
        match workspace {
            Ok(work_dir) => {
                if let Some(work_dir) = work_dir {
                    state
                        .db
                        .update_session_work_dir(&session.id, Some(&work_dir))
                        .await?;
                    session.work_dir = Some(work_dir);
                }
                return Ok(session);
            }
            Err(e) => {
                let _ = state.db.delete_session(&session.id).await;
                return Err(e);
            }
        }
    }

    let metadata = serde_json::json!({ "source": "feishu" }).to_string();
    // 有在线且接受远程任务的桌面 client 时，会话绑定到该 client 本地执行
    let client = crate::remote_exec::pick_online_client(state, user_id).await;
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
        assert_eq!(detect_image_media_type(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(detect_image_media_type(b"not-an-image"), None);
    }

    #[test]
    fn resolve_mentions_replaces_placeholders() {
        let mentions = vec![
            Mention { key: "@_user_1".into(), name: Some("MyBot".into()) },
            Mention { key: "@_user_2".into(), name: Some("运营专家".into()) },
        ];
        assert_eq!(
            resolve_mentions("@_user_1 帮我看看 @_user_2 的代码", &mentions),
            "@MyBot 帮我看看 @运营专家 的代码"
        );
    }

    #[test]
    fn resolve_mentions_skips_nameless() {
        let mentions = vec![Mention { key: "@_user_1".into(), name: None }];
        assert_eq!(resolve_mentions("@_user_1 你好", &mentions), "@_user_1 你好");
    }

    #[test]
    fn parse_commands() {
        assert_eq!(parse_command("/new"), Some(SlashCommand::New));
        assert_eq!(parse_command("/stop"), Some(SlashCommand::Stop));
        assert_eq!(parse_command("/status"), Some(SlashCommand::Status));
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
        assert_eq!(parse_command("@Agent OS /status"), Some(SlashCommand::Status));
        assert_eq!(parse_command("帮我运行 /status"), None);
        assert_eq!(parse_command("怎么使用 help"), None);
        assert_eq!(parse_command("/unknown"), None);
        assert_eq!(parse_command("@MyBot"), None);
    }

    #[test]
    fn parses_new_topic_workspace_decisions() {
        assert_eq!(
            parse_new_topic_decision(
                r#"{"agent_kind":"trace_code","agent_backend":"codex"}"#
            )
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
            parse_new_topic_decision(
                r#"{"agent_kind":"conversation","agent_backend":"native"}"#
            )
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
        assert_eq!(archive_inbound_content(&message("text", "bind 123456")), "bind ******");
        assert_eq!(archive_inbound_content(&message("text", "绑定需求")), "绑定需求");
        assert_eq!(archive_inbound_content(&message("image", "")), "[image message]");
    }
}
