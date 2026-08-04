//! 消息路由：事件解析 → 用户映射 → 斜杠命令 / 任务派发。
//!
//! 复刻 docs/book/agent-os 第 04/06 篇：
//! - text/post 正文提取、@_user_N 提及还原、thread_id/root_id 话题定位
//! - 一个话题 = 一个会话（feishu_chats 映射，topic = thread_id || root_id || "main"）
//! - /new /stop /status /nodes /help 命令管理当前话题会话

use crate::chat::{run_chat_turn, ChatTurnOpts};
use crate::feishu::api::FeishuApi;
use crate::feishu::card::{build_task_card, build_task_title, TaskCardOptions, TaskStatus};
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

/// 从消息 content 提取纯文本。
///
/// text 直接取；post 遍历富文本段落，取 text / a / code / code_block / md 的正文，
/// br 转换行，at 转 `@显示名`。代码块必须收——用户常把报错日志粘成 code_block。
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
                        // code_block / code / a / md 都带 text 字段，语义上都是用户输入的
                        // 正文。漏掉 code_block 会让「贴一段报错日志 @机器人」变成空文本，
                        // 然后被当成"不支持的消息类型"回绝——静默丢用户输入比报错更糟。
                        Some("text") | Some("a") | Some("code") | Some("code_block")
                        | Some("md") => {
                            if let Some(t) = el["text"].as_str() {
                                out.push_str(t);
                            }
                        }
                        // 富文本换行是独立元素，不补的话多行日志会被拼成一整行。
                        Some("br") => out.push('\n'),
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
    /// A 股研究话题：server 侧 native，只挂 quant_* 工具，无工作区。
    QuantResearch,
    TraceCode,
    QuantCode,
    GeneralTask,
}

impl AgentKind {
    fn agent_kind(&self) -> &'static str {
        match self {
            Self::Conversation => "conversation",
            Self::QuantResearch => "quant_research",
            Self::TraceCode => "trace_code",
            Self::QuantCode => "quant_code",
            Self::GeneralTask => "general_task",
        }
    }

    fn workspace_kind(&self) -> WorkspaceKind {
        match self {
            // quant_research 与 conversation 一样不建目录：只有 REST 研究工具，无 cwd。
            Self::Conversation | Self::QuantResearch => WorkspaceKind::None,
            Self::TraceCode | Self::QuantCode => WorkspaceKind::Repository,
            Self::GeneralTask => WorkspaceKind::General,
        }
    }
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct NewTopicDecision {
    agent_kind: AgentKind,
}

impl NewTopicDecision {
    fn fallback() -> Self {
        Self {
            agent_kind: AgentKind::GeneralTask,
        }
    }

    fn agent_kind(&self) -> &'static str {
        self.agent_kind.agent_kind()
    }

    fn workspace_kind(&self) -> WorkspaceKind {
        self.agent_kind.workspace_kind()
    }
}

pub fn parse_command(text: &str) -> Option<SlashCommand> {
    let normalized = text.trim().to_ascii_lowercase();
    let t = normalized.as_str();
    const COMMANDS: [(&str, SlashCommand); 4] = [
        ("/new", SlashCommand::New),
        ("/stop", SlashCommand::Stop),
        ("/status", SlashCommand::Status),
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
        return handle_command(&state, &api, &account, &msg, cmd).await;
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
    cmd: SlashCommand,
) -> Result<()> {
    let topic = msg.topic_id();
    match cmd {
        SlashCommand::Help => {
            api.reply_text(
                &msg.message_id,
                "直接问行情、信号、回测即可（quant 研究话题）\n/new 开启新话题\n/stop 停止当前任务\n/status 查看当前会话\n/help 查看命令",
                msg.in_thread(),
            )
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
    }
    Ok(())
}

// ── 任务派发 ──

/// 新话题先判断是否真的需要工作区，以及工作区是否属于 Trace monorepo。
/// 分类失败时降级到 general_task，绝不静默建 server worktree。
///
/// 外部代码 Agent 已下线，所有会话都在 server 侧 native 执行。
async fn decide_new_topic(state: &AppState, text: &str) -> NewTopicDecision {
    let server_agent_enabled = state.config.server_agent.enabled;
    // quant_a2a 关闭时路由 prompt 不得出现 quant_research，避免模型凭常识乱猜该类型。
    let quant_enabled = state.config.quant_a2a.as_ref().is_some_and(|c| c.enabled);
    match try_decide_new_topic(state, text, server_agent_enabled, quant_enabled).await {
        Ok(decision) => {
            tracing::info!(
                ?decision,
                server_agent_enabled,
                quant_enabled,
                "feishu: new topic workspace decision"
            );
            decision
        }
        Err(e) => {
            tracing::warn!("feishu: workspace decision failed, fallback to general: {e:#}");
            NewTopicDecision::fallback()
        }
    }
}

async fn try_decide_new_topic(
    state: &AppState,
    text: &str,
    server_agent_enabled: bool,
    quant_enabled: bool,
) -> Result<NewTopicDecision> {
    let (record, provider) = provider_registry::resolve_default(&state.db)
        .await
        .ok_or_else(|| anyhow!("没有可用的 LLM provider"))?;
    // 代码/文件任务需要 server 侧工作区；server_agent 关闭时只能纯对话。
    let env_note = if server_agent_enabled {
        "当前环境已开启 server_agent：所有会话都在 server 侧执行，没有用户本机执行通道。\
         代码/文件任务会在 server 的工作区里读改文件、跑命令。"
    } else {
        "当前环境未开启 server_agent，没有 server 侧代码工作区。\
         凡需要读改代码、跑命令、操作文件的任务都无法执行，这类消息一律归到 conversation，\
         由对话 Agent 说明限制。"
    };
    // quant_research 只在 quant_a2a.enabled 时进入可选列表；关闭时保持与历史 prompt 一致（无回归）。
    let quant_research_line = if quant_enabled {
        "- quant_research：用户在问 A 股行情、信号、选股、策略、因子、回测、持仓记账，\
          或要求验证/研究某个量化想法。这类任务由 quant 研究工具直接回答，不需要\
          读写代码文件。注意：修改 quant 项目代码本身属于 quant_code，不是 quant_research。\n"
            .to_string()
    } else {
        String::new()
    };
    let quant_priority_note = if quant_enabled {
        "只要用户是在用 quant 的数据和能力做研究（查信号、跑回测、评估因子），\
         一律选 quant_research；只有当用户要改 quant 的源码、看板或文档时才选 quant_code。\n"
            .to_string()
    } else {
        // enabled=false 时显式禁止，防止模型凭常识乱猜该类型。
        "本环境没有 quant 研究工具，不得输出 quant_research。\n".to_string()
    };
    let system = format!("你是飞书任务的路由 Agent。只输出一个 JSON 对象，不要输出 markdown 或其他文字。\n\
        {env_note}\n\
        输出字段 agent_kind 可选值：\n\
        - trace_code：需要读取、修改、测试或部署 Trace/Hank monorepo 的 server、\
          crates、admin、docs、飞书/微信渠道或同步流程；不包括 client 和 quant。\n\
        - quant_code：需要读取、修改或测试独立 quant 仓库（github.com/hankjs/quant）的代码、策略、看板或文档。\n\
        - general_task：具体任务与 Trace/quant 无关，但需要文件、代码、命令、下载、分析产物或持续迭代工作区。\n\
        {quant_research_line}\
        - conversation：用户在问候、讨论、咨询、分析问题，或者尚未给出需要文件和命令的事项。\
          后续对话 Agent 会负责正式回答；路由器不要回答用户问题。\n\
        只输出 agent_kind 一个字段。示例：{{\"agent_kind\":\"trace_code\"}}。\n\
        {quant_priority_note}\
        判断 Agent 必须看语义，不只看是否出现项目名。拿不准是否属于 Trace/quant 时选择 general_task；\
        拿不准是否需要文件或命令时选择 conversation。",
        env_note = env_note,
        quant_research_line = quant_research_line,
        quant_priority_note = quant_priority_note,
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

    // 卡片标题：优先用 content 里的文本；纯图片等多模态块取不到文本时传空串，
    // build_task_title 会回落「Agent 任务」。首响卡与 pusher 共用同一值，
    // 必须在 content move 进 run_chat_turn 之前取。
    let task_title: String = content
        .iter()
        .filter_map(|block| match block {
            hank_provider::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");

    // 新话题的路由分类要 60~90s，期间没有 session_id 可用于 try_acquire。
    // 用话题 key 占位，避免用户重复发送时起第二个 run（线上实测会冒出两张卡）。
    // try_acquire 接受任意字符串 key，这里与 session 级共用同一套 API。
    let topic_key = format!("{}:{}:{}", account.id, msg.chat_id, topic);
    let Some(topic_guard) = state.tasks.try_acquire(&topic_key).await else {
        // 尚无 session_id，不能用 running_reply；固定文案即可。
        api.reply_text(&msg.message_id, "上一条还在处理中，请稍候", msg.in_thread())
            .await?;
        return Ok(());
    };

    // 首响：路由 Agent 分类要调 LLM（实测 60~90s），必须先给用户可见反馈，
    // 否则用户以为机器人没收到、重复发送 → 冒出多张卡片。
    // 这张卡后续由 pusher 原地更新为运行中/终态，不新增消息。
    // 发卡失败不阻断任务，退化为原有行为（pusher 自己新建）。
    let ack_card_id = api
        .reply_card(
            &msg.message_id,
            &build_task_card(&TaskCardOptions {
                title: build_task_title(&task_title),
                status: TaskStatus::Received,
                progress: 0,
                detail: "已收到，正在判断任务类型".to_string(),
                activities: vec![],
                footer: None,
                actions: vec![],
                session_id: String::new(), // 尚未有 session
                chat_id: msg.chat_id.clone(),
                topic_id: topic.clone(),
            }),
            msg.in_thread(),
        )
        .await
        .ok();

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
        Ok(None) => {
            // topic_guard Drop 释放；ack 卡可能仍在「已收到」，属快速路径放弃
            return Ok(());
        }
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
    // 话题级占位在 session 级名额拿到后立刻释放，避免两层互相卡住。
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
    // 拿到 session 级名额后立刻释放话题级，后续并发由 session 级 + active_tasks 挡。
    topic_guard.release().await;

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
                task_title,
                ack_card_id,
                msg.in_thread(),
                handle.event_rx,
            );
        }
        Err(e) => {
            tracing::warn!("feishu: run_chat_turn failed: {e}");
            state.tasks.clear_progress(&session_id).await;
            let reply = match &e {
                crate::chat::ChatTurnError::UserFacing(msg) => msg.clone(),
                _ => format!("启动失败：{e}"),
            };
            api.reply_text(&msg.message_id, &reply, msg.in_thread())
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

async fn create_and_map_feishu_session(
    state: &Arc<AppState>,
    account: &FeishuAccount,
    msg: &IncomingMessage,
    topic: &str,
    user_id: &str,
    content: &[ContentBlock],
) -> Result<Option<String>> {
    let decision = decide_new_topic(state, &classification_text(content)).await;
    let session = create_feishu_session(
        state,
        user_id,
        decision.agent_kind(),
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

/// 历史外部 CLI 后端名单。这四个 backend 已随外部代码 Agent 一起下线，
/// 只在识别老会话 metadata 时还需要——不能再当作可执行后端。
const RETIRED_EXTERNAL_BACKENDS: [&str; 4] = ["codex", "claude", "grok", "kimi"];

fn is_external_agent_backend(backend: &str) -> bool {
    RETIRED_EXTERNAL_BACKENDS.contains(&backend)
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
    // 历史 client-only 话题绑的是已下线的本机 hank-cli，不能静默复用。
    if is_client_only_metadata(&value) && is_external_agent_backend(backend) {
        return SessionReusePolicy::RequireNew {
            backend: backend.to_string(),
            reason: "external_agent_retired",
        };
    }
    if is_client_only_metadata(&value) {
        return SessionReusePolicy::ReuseManaged;
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
    /// 可直接复用的 managed 会话（native）。
    ReuseManaged,
    /// 历史外部后端会话（server-agent 或 client-only）：禁止静默复用，要求 /new。
    RequireNew {
        backend: String,
        reason: &'static str,
    },
    /// 未管理会话：删除映射后重建。
    Recreate,
}

fn legacy_server_agent_require_new_message(backend: &str) -> String {
    format!(
        "该话题绑定的外部代码 Agent（backend={backend}）已下线，无法继续执行。请发送 /new 开启新话题。"
    )
}

/// 飞书 conversation 会话的链路架构说明。
fn feishu_conversation_architecture_text() -> &'static str {
    "【飞书链路与执行架构】\n\
     - 用户通过飞书对话，消息经 Trace server 转发。\n\
     - 所有会话都在 Trace server 侧执行；外部代码 Agent（codex / claude / grok / kimi）与\
用户本机执行节点已下线，server 不会在用户电脑上跑任何命令。\n\
     - 一个飞书话题 = 一个会话；换话题需要 /new。\n\
     - 可用命令：/new /stop /status /help。\n\
     - 当前话题是纯对话模式，没有工作目录也没有执行工具；用户要跑命令或改代码时应说明当前不支持。"
}

async fn build_feishu_conversation_extra_prompts(_state: &AppState, _user_id: &str) -> Vec<String> {
    vec![feishu_conversation_architecture_text().to_string()]
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
    workspace_kind: WorkspaceKind,
) -> Result<hank_db::Session> {
    // quant 研究话题：server 侧 native 会话，只挂 quant_* 工具，无工作区、
    // 不绑执行节点，因此不要求 can_login_admin（admin 边界只在创建 server 工作区时校验）。
    // 必须写 source=feishu：chat.rs 用它决定确认话术与是否允许「确认N次」批量授权。
    // 不要写 server_agent=true，否则会注入「你正在 wananyun 工作区」的错误说明。
    if agent_kind == "quant_research" {
        let metadata = serde_json::json!({
            "source": "feishu",
            "agent_backend": "native",
            "agent_kind": agent_kind,
            "workspace_kind": "none",
        })
        .to_string();
        let session = state
            .db
            .create_session(
                "",
                "",
                None,
                Some(user_id),
                Some("remote"),
                Some("chat"),
                Some(&metadata),
            )
            .await
            .map_err(|e| anyhow!("create quant research session: {e:#}"))?;
        return Ok(session);
    }

    // server-agent 开启时建无 worktree 的 server 会话。
    if state.config.server_agent.enabled {
        crate::server_workspace::ensure_server_agent_admin(state, user_id).await?;
        let metadata = serde_json::json!({
            "source": "feishu",
            "server_agent": true,
            "agent_backend": "native",
            "agent_kind": agent_kind,
            "workspace_kind": workspace_kind.as_str(),
            "client_excluded": true,
        })
        .to_string();
        let session = state
            .db
            .create_session(
                "",
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
            // 代码/文件任务的 server 工作区已下线；落到此路径不建 worktree。
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
    let session = state
        .db
        .create_session(
            "",
            "",
            None,
            Some(user_id),
            Some("remote"),
            Some("chat"),
            Some(&metadata),
        )
        .await
        .map_err(|e| anyhow!("create session: {e:#}"))?;
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
    fn extract_text_from_post_with_code_block() {
        // 用户把报错日志粘成代码块：漏掉 code_block 会让文本变空，
        // 然后被当成"不支持的消息类型"回绝。
        let content = r#"{"title":"","content":[[{"tag":"text","text":"这个报错怎么修 "},{"tag":"code_block","text":"panic at line 3"}]]}"#;
        assert_eq!(
            extract_text("post", content),
            "这个报错怎么修 panic at line 3"
        );
    }

    #[test]
    fn extract_text_from_post_with_link_md_and_br() {
        let content = r#"{"title":"","content":[[{"tag":"a","text":"文档","href":"https://x"},{"tag":"br"},{"tag":"code","text":"cargo test"},{"tag":"md","text":" 看这里"}]]}"#;
        assert_eq!(extract_text("post", content), "文档\ncargo test 看这里");
    }

    #[test]
    fn extract_text_from_post_ignores_unknown_tag() {
        let content = r#"{"title":"","content":[[{"tag":"text","text":"前"},{"tag":"emotion","emoji_type":"SMILE"},{"tag":"text","text":"后"}]]}"#;
        assert_eq!(extract_text("post", content), "前后");
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
        assert_eq!(parse_command("帮我运行 /status"), None);
        // 已移除的命令不再被识别（曾经是 /nodes /diff /test /deploy /rollback）
        assert_eq!(parse_command("/nodes"), None);
        assert_eq!(parse_command("/deploy"), None);
        assert_eq!(parse_command("怎么使用 help"), None);
        assert_eq!(parse_command("/unknown"), None);
        assert_eq!(parse_command("@MyBot"), None);
    }

    #[test]
    fn architecture_text_states_server_side_only_execution() {
        let text = feishu_conversation_architecture_text();
        // 外部 CLI 已下线，文案必须明确"不在用户电脑上执行"，否则模型会承诺做不到的事
        assert!(text.contains("Trace server 侧执行"), "{text}");
        assert!(text.contains("已下线"), "{text}");
        for cmd in ["/new", "/stop", "/status", "/help"] {
            assert!(text.contains(cmd), "{cmd} missing: {text}");
        }
        assert!(text.contains("纯对话"), "{text}");
    }

    #[test]
    fn quant_research_decision_parsing() {
        // 历史 prompt 会带 agent_backend，多余字段必须被忽略而不是解析失败
        assert_eq!(
            parse_new_topic_decision(r#"{"agent_kind":"quant_research","agent_backend":"native"}"#)
                .unwrap(),
            NewTopicDecision {
                agent_kind: AgentKind::QuantResearch,
            }
        );
        assert_eq!(
            AgentKind::QuantResearch.workspace_kind(),
            WorkspaceKind::None
        );
    }

    #[test]
    fn parses_new_topic_workspace_decisions() {
        assert_eq!(
            parse_new_topic_decision(r#"{"agent_kind":"trace_code"}"#).unwrap(),
            NewTopicDecision {
                agent_kind: AgentKind::TraceCode,
            }
        );
        assert_eq!(
            parse_new_topic_decision("```json\n{\"agent_kind\":\"quant_code\"}\n```").unwrap(),
            NewTopicDecision {
                agent_kind: AgentKind::QuantCode,
            }
        );
        assert_eq!(
            parse_new_topic_decision(r#"{"agent_kind":"conversation"}"#)
                .unwrap()
                .agent_kind(),
            "conversation"
        );
        assert_eq!(
            NewTopicDecision::fallback().workspace_kind(),
            WorkspaceKind::General
        );
        assert_eq!(
            AgentKind::QuantCode.workspace_kind(),
            WorkspaceKind::Repository
        );
    }

    #[test]
    fn legacy_external_backend_sessions_require_new() {
        // server-agent 与 client-only 两条历史路径都不能静默复用，否则用户以为任务还在跑
        for backend in ["codex", "claude", "grok", "kimi"] {
            let cases = [
                (
                    format!(
                        r#"{{"server_agent":true,"agent_backend":"{backend}","workspace_kind":"repository"}}"#
                    ),
                    "legacy_server_agent",
                ),
                (
                    format!(
                        r#"{{"agent_location":"client","agent_backend":"{backend}","exec_client_id":"cli-1"}}"#
                    ),
                    "external_agent_retired",
                ),
            ];
            for (metadata, reason) in cases {
                assert_eq!(
                    reuse_policy_for_session_metadata(&metadata),
                    SessionReusePolicy::RequireNew {
                        backend: backend.into(),
                        reason,
                    },
                    "backend={backend} reason={reason}"
                );
            }
            let message = legacy_server_agent_require_new_message(backend);
            assert!(message.contains("/new"), "{message}");
            assert!(message.contains(backend), "{message}");
            assert!(message.contains("已下线"), "{message}");
        }
    }

    #[test]
    fn legacy_client_only_native_session_can_reuse() {
        assert_eq!(
            reuse_policy_for_session_metadata(
                r#"{"agent_location":"client","agent_backend":"native"}"#
            ),
            SessionReusePolicy::ReuseManaged
        );
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
