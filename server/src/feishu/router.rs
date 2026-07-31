//! 消息路由：事件解析 → 用户映射 → 斜杠命令 / 任务派发。
//!
//! 复刻 docs/book/agent-os 第 04/06 篇：
//! - text/post 正文提取、@_user_N 提及还原、thread_id/root_id 话题定位
//! - 一个话题 = 一个会话（feishu_chats 映射，topic = thread_id || root_id || "main"）
//! - /new /stop /status /help 命令管理当前话题会话

use crate::chat::{run_chat_turn, ChatTurnOpts};
use crate::feishu::api::FeishuApi;
use crate::feishu::pusher;
use crate::AppState;
use anyhow::{anyhow, Result};
use hank_db::FeishuAccount;
use serde::Deserialize;
use std::sync::Arc;

// ── 事件结构（只取需要的字段，容错未知字段）──

#[derive(Debug, Deserialize)]
struct EventEnvelope {
    event: EventData,
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
    Help,
}

pub fn parse_command(text: &str) -> Option<SlashCommand> {
    let t = text.trim();
    const COMMANDS: [(&str, SlashCommand); 4] = [
        ("/new", SlashCommand::New),
        ("/stop", SlashCommand::Stop),
        ("/status", SlashCommand::Status),
        ("/help", SlashCommand::Help),
    ];
    for (pat, cmd) in COMMANDS {
        if t == pat {
            return Some(cmd);
        }
        // "@提及 /cmd" 形式：以 @ 开头、以命令结尾、命令前是空白
        if t.starts_with('@') && t.ends_with(pat) {
            let prefix = &t[..t.len() - pat.len()];
            if prefix.ends_with(char::is_whitespace) {
                return Some(cmd);
            }
        }
    }
    None
}

// ── 事件入口 ──

pub async fn handle_event(state: Arc<AppState>, account: FeishuAccount, payload: &[u8]) -> Result<()> {
    // 先只取 header.event_type，避免无关事件被完整反序列化卡住
    let header: serde_json::Value = serde_json::from_slice(payload)?;
    let event_type = header["header"]["event_type"].as_str().unwrap_or("");
    match event_type {
        "im.message.receive_v1" => {
            let envelope: EventEnvelope = serde_json::from_slice(payload)?;
            handle_message(state, account, envelope.event).await
        }
        other => {
            tracing::debug!(event_type = other, "feishu: ignore event");
            Ok(())
        }
    }
}

async fn handle_message(state: Arc<AppState>, account: FeishuAccount, data: EventData) -> Result<()> {
    // 忽略 bot 自己/其他应用的消息，防自循环
    if data.sender.sender_type.as_deref() != Some("user") {
        return Ok(());
    }
    let m = data.message;
    let mentions: Vec<Mention> = m
        .mentions
        .unwrap_or_default()
        .into_iter()
        .map(|me| Mention { key: me.key, name: me.name })
        .collect();
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
            chrono::Utc::now(),
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

    // 斜杠命令
    if let Some(cmd) = parse_command(&msg.text) {
        return handle_command(&state, &api, &account, &msg, cmd).await;
    }

    if msg.text.is_empty() {
        // 图片/文件等富媒体二期再解析；空文本不派发，避免空 prompt 白跑一轮
        if msg.message_type != "text" {
            api.reply_text(
                &msg.message_id,
                "收到，但暂时只支持文字消息（图片/文件解析二期上线）",
                msg.in_thread(),
            )
            .await?;
        }
        return Ok(());
    }

    dispatch_task(&state, &api, &account, &msg, &user_id, &msg.text.clone()).await
}

/// 未绑定用户：bind <6位码> 或提示（与微信同一绑定码模式）。
async fn handle_unbound(
    state: &Arc<AppState>,
    api: &FeishuApi,
    account: &FeishuAccount,
    msg: &IncomingMessage,
) {
    let code = msg
        .text
        .strip_prefix("bind")
        .map(str::trim)
        .filter(|c| c.len() == 6 && c.chars().all(|c| c.is_ascii_digit()))
        .map(|c| c.to_string());
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
                "/new 开启新会话\n/stop 停止当前任务\n/status 查看当前会话\n/help 查看命令",
                msg.in_thread(),
            )
            .await?;
        }
        SlashCommand::Status => {
            let chat = state.db.get_feishu_chat(&account.id, &msg.chat_id, &topic).await?;
            let text = match chat {
                Some(c) => {
                    let running = state.active_tasks.read().await.contains_key(&c.session_id);
                    format!(
                        "会话：{}\n状态：{}\n话题：{}",
                        c.session_id,
                        if running { "执行中" } else { "空闲" },
                        topic
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
    }
    Ok(())
}

// ── 任务派发 ──

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
    let topic = msg.topic_id();

    // 找/建 feishu_chats 映射的 session
    let session_id = match state.db.get_feishu_chat(&account.id, &msg.chat_id, &topic).await {
        Ok(Some(c)) => c.session_id,
        _ => match create_feishu_session(state, user_id).await {
            Ok(session) => {
                if let Err(e) = state
                    .db
                    .set_feishu_chat(&account.id, &msg.chat_id, &topic, &session.id, user_id)
                    .await
                {
                    tracing::warn!("feishu: set chat failed: {e:#}");
                }
                session.id
            }
            Err(e) => {
                tracing::warn!("feishu: create session failed: {e:#}");
                api.reply_text(&msg.message_id, "创建会话失败，请稍后重试", msg.in_thread())
                    .await?;
                return Ok(());
            }
        },
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
    let content = vec![hank_provider::ContentBlock::Text {
        text: text.to_string(),
    }];
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

async fn create_feishu_session(state: &Arc<AppState>, user_id: &str) -> Result<hank_db::Session> {
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
        assert_eq!(parse_command("/help"), Some(SlashCommand::Help));
        assert_eq!(parse_command("@MyBot /status"), Some(SlashCommand::Status));
        assert_eq!(parse_command("@Agent OS /status"), Some(SlashCommand::Status));
        assert_eq!(parse_command("帮我运行 /status"), None);
        assert_eq!(parse_command("/unknown"), None);
        assert_eq!(parse_command("@MyBot"), None);
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
