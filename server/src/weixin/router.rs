//! 微信消息路由：绑定检查 / bind 码 / 斜杠命令 / 渠道 agent 决策分发。

use crate::chat::{run_chat_turn, ChatTurnOpts};
use crate::weixin::api::{IlinkClient, IlinkMessage};
use crate::weixin::channel::{self, ChannelAction};
use crate::weixin::pusher;
use crate::AppState;
use anyhow::{anyhow, Result};
use hank_db::{WeixinAccount, WeixinBinding};
use std::sync::Arc;

const BIND_CODE_TTL_MS: i64 = 10 * 60 * 1000;

/// 为绑定用户签发短期 JWT（1 小时），供 spec 类工具回调 server 使用。
fn sign_internal_jwt(state: &AppState, user_id: &str, username: &str) -> Result<String> {
    Ok(crate::auth::sign_internal_jwt(
        &state.jwt_secret,
        user_id,
        username,
    )?)
}

pub async fn handle_message(
    state: Arc<AppState>,
    account: WeixinAccount,
    msg: IlinkMessage,
) -> Result<()> {
    let from = msg.from_user_id.clone().unwrap_or_default();
    let context_token = match msg.context_token.clone() {
        Some(t) if !t.is_empty() => t,
        _ => {
            tracing::warn!(from, "weixin: message without context_token, cannot reply");
            return Ok(());
        }
    };
    let client = IlinkClient::new();
    let reply = |text: &str| {
        let client = client.clone();
        let account = account.clone();
        let from = from.clone();
        let token = context_token.clone();
        let text = text.to_string();
        async move {
            if let Err(e) = client.send_text(&account, &from, &token, &text).await {
                tracing::warn!("weixin: reply failed: {e:#}");
            }
        }
    };

    // 已绑定则顺手刷新 context_token
    let binding = state
        .db
        .get_weixin_binding_by_bot(&account.ilink_bot_id, &from)
        .await
        .unwrap_or(None);
    if let Some(ref b) = binding {
        if b.context_token.as_deref() != Some(context_token.as_str()) {
            let _ = state
                .db
                .update_weixin_binding_context(&b.id, &context_token)
                .await;
        }
    }

    let text = match msg.text() {
        Some(t) => t.trim().to_string(),
        None => {
            reply("暂只支持文本消息").await;
            return Ok(());
        }
    };

    let binding = match binding {
        Some(b) => b,
        None => {
            handle_unbound(&state, &account, &from, &text, reply).await;
            return Ok(());
        }
    };

    // bind 是一次性绑定流程。重复投递或用户再次发送绑定码都必须停在
    // 渠道层，不能因为“已经绑定”而把原始 bind 文本派发给 coding agent。
    if parse_bind_code(&text).is_some() {
        reply("这个微信已经绑定 Trace 账号，直接发消息即可开始对话").await;
        return Ok(());
    }

    // /ai <text>：跳过渠道 agent，直接派发给 coding agent
    if let Some(task) = text.strip_prefix("/ai") {
        let task = task.trim();
        if !task.is_empty() {
            channel::push_history(&state, &binding.id, task, "收到，任务已派发").await;
            dispatch_task(
                &state,
                &account,
                &binding,
                &from,
                &context_token,
                task,
                &reply,
            )
            .await;
            return Ok(());
        }
    }

    // 不带斜杠的菜单请求：直接回固定文案，避免渠道 agent 凭印象编功能清单
    let trimmed = text.trim();
    if trimmed == "菜单"
        || trimmed == "帮助"
        || trimmed.eq_ignore_ascii_case("menu")
        || trimmed.eq_ignore_ascii_case("help")
    {
        reply(MENU_TEXT).await;
        return Ok(());
    }

    // 斜杠命令
    if text.starts_with('/') {
        handle_command(
            &state,
            &account,
            &binding,
            &from,
            &context_token,
            &text,
            reply,
        )
        .await;
        return Ok(());
    }

    handle_chat(
        &state,
        &account,
        &binding,
        &from,
        &context_token,
        &text,
        &reply,
    )
    .await;
    Ok(())
}

/// 未绑定用户：bind <6位码> 或提示。
async fn handle_unbound<Fut: std::future::Future<Output = ()>>(
    state: &Arc<AppState>,
    account: &WeixinAccount,
    from: &str,
    text: &str,
    reply: impl FnOnce(&str) -> Fut,
) {
    let code = match parse_bind_code(text) {
        Some(c) => c.to_string(),
        None => {
            reply("请先在 Trace client 生成绑定码，然后发送 bind 123456").await;
            return;
        }
    };
    match state.db.consume_weixin_bind_code(&code).await {
        Ok(Some(user_id)) => {
            match state
                .db
                .create_weixin_binding(&account.id, from, &user_id)
                .await
            {
                Ok(_binding_id) => {
                    tracing::info!(user_id, ilink_user_id = from, "weixin binding created");
                    reply("绑定成功！直接发送消息即可开始对话，发送 /菜单 查看全部能力").await;
                }
                Err(e) => {
                    tracing::warn!("weixin: create binding failed: {e:#}");
                    reply("绑定失败，请稍后重试").await;
                }
            }
        }
        Ok(None) => reply("绑定码无效或已过期，请在 Trace client 重新生成").await,
        Err(e) => {
            tracing::warn!("weixin: consume bind code failed: {e:#}");
            reply("绑定失败，请稍后重试").await;
        }
    }
}

fn parse_bind_code(text: &str) -> Option<&str> {
    let code = text.trim().strip_prefix("bind")?.trim();
    (code.len() == 6 && code.chars().all(|c| c.is_ascii_digit())).then_some(code)
}

/// 斜杠命令：/new /stop /status /terms /term /send /shot /snap。
/// account/from/context_token 用于截图类命令直接发图（send_media）。
/// 注意：命令及其输出一律不进渠道对话记忆（规范见 channel.rs 模块文档）；
/// 唯一例外是 /new 成功时清空记忆。
async fn handle_command<Fut: std::future::Future<Output = ()>>(
    state: &Arc<AppState>,
    account: &WeixinAccount,
    binding: &WeixinBinding,
    from: &str,
    context_token: &str,
    text: &str,
    reply: impl FnOnce(&str) -> Fut,
) {
    let cmd = text.split_whitespace().next().unwrap_or("");
    let session_id = state.db.get_weixin_chat(&binding.id).await.ok().flatten();
    match cmd {
        "/new" => match new_weixin_session(state, binding).await {
            Ok(()) => {
                channel::clear_history(state, &binding.id).await;
                reply("已开启新会话").await
            }
            Err(e) => {
                tracing::warn!("weixin: create session failed: {e:#}");
                reply("创建会话失败，请稍后重试").await;
            }
        },
        "/stop" => {
            if stop_current_task(state, binding).await {
                reply("已停止当前任务").await;
            } else {
                reply("当前没有正在执行的任务").await;
            }
        }
        "/status" => {
            let msg = match &session_id {
                Some(sid) => {
                    let running = state.active_tasks.read().await.contains_key(sid);
                    let session = state.db.get_session(sid).await.ok().flatten();
                    let title = session
                        .as_ref()
                        .map(|s| s.title.clone())
                        .unwrap_or_default();
                    let title = if title.is_empty() {
                        "(无标题)"
                    } else {
                        title.as_str()
                    };
                    let mut text = format!(
                        "会话：{title}\nID：{sid}\n状态：{}",
                        if running { "执行中" } else { "空闲" }
                    );
                    // 执行中就把实时进度一并带上，省得再问一次
                    if running {
                        if let Some(snapshot) = state.tasks.progress(sid).await {
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
                None => "当前没有会话，直接发送消息即可开始".to_string(),
            };
            reply(&msg).await;
        }
        "/snap" => {
            let msg = web_snap(state, account, from, context_token, text).await;
            reply(&msg).await;
        }
        "/菜单" | "/menu" | "/help" | "/帮助" => reply(MENU_TEXT).await,
        _ => reply("未知命令，发送 /菜单 查看全部能力").await,
    }
}

async fn web_snap(
    state: &Arc<AppState>,
    account: &WeixinAccount,
    from: &str,
    context_token: &str,
    text: &str,
) -> String {
    let url = text.split_whitespace().nth(1);
    let Some(url) = url else {
        return "用法：/snap <url>（仅支持 http/https）".to_string();
    };
    let client = IlinkClient::new();
    if let Err(e) = client
        .send_text(account, from, context_token, "网页截图中，请稍候…")
        .await
    {
        tracing::warn!("weixin: snap ack reply failed: {e:#}");
    }
    let png = match crate::websnap::snap_url(state.config.server.chrome_path.as_deref(), url).await
    {
        Ok(png) => png,
        Err(e) => return format!("网页截图失败：{e:#}"),
    };
    match client
        .send_media(account, from, context_token, "snap.png", &png)
        .await
    {
        Ok(()) => format!("网页截图已发送：{url}"),
        Err(e) => {
            tracing::warn!("weixin: send web snap failed: {e:#}");
            format!("截图发送失败：{e:#}")
        }
    }
}

/// 派发任务时注入的渠道提示：告知 agent 媒体文件回传约定。
const WEIXIN_FILE_HINT: &str = "\n\n（渠道提示：本任务来自微信。如需把生成的文件（图片、图表、文档等）发给用户，\
在最终回复中单独写 [file:/绝对/路径]，每个文件一条，系统会自动把文件发到微信。不要虚构路径，只标记真实存在的文件。）";

/// /菜单 命令的能力说明文案。
const MENU_TEXT: &str = "\
我是 Trace 微信助手，可以帮你远程驱动 AI 编程会话：

【对话能力】
· 直接发消息 = 给 agent 派任务（写代码、查问题、分析项目都行）
· 执行中会推送进度摘要，完成后发回完整结果
· 接着发消息 = 在同一会话里追问、补充要求
· agent 向你提问时，直接回复即可作答
· agent 生成的图片/文件可直接发回微信

【命令】
/new — 开一个新会话（旧话题结束）
/stop — 停止正在执行的任务
/status — 查看当前会话和执行状态
/ai <任务> — 跳过智能判断，直接派任务给 agent
/菜单 — 显示本说明

【截图】
/snap <url> — 网页截图（图片）

【小贴士】
· 任务在服务器端执行
· 同一时间只能跑一个任务，执行中新消息会排队提醒
· 也可以直接说「截图 xxx 官网」，agent 会自动截好发你
· 暂只支持文字输入，语音还在开发中";

/// 普通文本：先由渠道 agent 决策，再按 action 分发。
/// 渠道 agent 失败时降级为直接 dispatch 原文（保持原有行为）。
async fn handle_chat<'a, Fut: std::future::Future<Output = ()>>(
    state: &'a Arc<AppState>,
    account: &'a WeixinAccount,
    binding: &'a WeixinBinding,
    from: &'a str,
    context_token: &'a str,
    text: &'a str,
    reply: &'a (impl Fn(&str) -> Fut + 'a),
) {
    let session_id = state.db.get_weixin_chat(&binding.id).await.ok().flatten();
    let history = channel::history(state, &binding.id).await;
    match channel::decide(state, binding, session_id.as_deref(), text, &history).await {
        Some(ChannelAction::Reply { text: t }) => {
            channel::push_history(state, &binding.id, text, &t).await;
            reply(&t).await
        }
        Some(ChannelAction::Dispatch { ack, task }) => {
            if !ack.trim().is_empty() {
                reply(&ack).await;
            }
            let ack_text = if ack.trim().is_empty() {
                "收到，任务已派发"
            } else {
                ack.trim()
            };
            channel::push_history(state, &binding.id, text, ack_text).await;
            let task = if task.trim().is_empty() {
                text
            } else {
                task.trim()
            };
            dispatch_task(state, account, binding, from, context_token, task, reply).await;
        }
        Some(ChannelAction::Stop { text: t }) => {
            let stopped = stop_current_task(state, binding).await;
            let msg = if !t.trim().is_empty() {
                t
            } else if stopped {
                "已停止当前任务".to_string()
            } else {
                "当前没有正在执行的任务".to_string()
            };
            channel::push_history(state, &binding.id, text, &msg).await;
            reply(&msg).await;
        }
        Some(ChannelAction::New { text: t }) => match new_weixin_session(state, binding).await {
            Ok(()) => {
                channel::clear_history(state, &binding.id).await;
                let msg = if t.trim().is_empty() {
                    "已开启新会话"
                } else {
                    t.trim()
                };
                reply(msg).await;
            }
            Err(e) => {
                tracing::warn!("weixin: create session failed: {e:#}");
                reply("创建会话失败，请稍后重试").await;
            }
        },
        None => {
            channel::push_history(state, &binding.id, text, "收到，任务已派发").await;
            dispatch_task(state, account, binding, from, context_token, text, reply).await;
        }
    }
}

/// 派发任务：找/建会话，跑一轮 chat，事件流交给 pusher 回推。
async fn dispatch_task<'a, Fut: std::future::Future<Output = ()>>(
    state: &'a Arc<AppState>,
    account: &'a WeixinAccount,
    binding: &'a WeixinBinding,
    from: &'a str,
    context_token: &'a str,
    text: &'a str,
    reply: &'a (impl Fn(&str) -> Fut + 'a),
) {
    // 找/建 weixin_chats 映射的 session
    let session_id = match state.db.get_weixin_chat(&binding.id).await {
        Ok(Some(sid)) => sid,
        _ => match create_weixin_session(state, binding).await {
            Ok(session) => {
                if let Err(e) = state.db.set_weixin_chat(&binding.id, &session.id).await {
                    tracing::warn!("weixin: set chat failed: {e:#}");
                }
                session.id
            }
            Err(e) => {
                tracing::warn!("weixin: create session failed: {e:#}");
                reply("创建会话失败，请稍后重试").await;
                return;
            }
        },
    };

    // 并发控制：同 session 同时只跑一个 turn。
    // active_tasks 要等 run_chat_turn 走完准备工作才登记，先原子抢派发名额堵住空窗。
    let Some(dispatch_guard) = state.tasks.try_acquire(&session_id).await else {
        reply(&running_reply(state, &session_id).await).await;
        return;
    };
    if state.active_tasks.read().await.contains_key(&session_id) {
        dispatch_guard.release().await;
        reply(&running_reply(state, &session_id).await).await;
        return;
    }

    let username = state
        .db
        .get_user_by_id(&binding.user_id)
        .await
        .ok()
        .flatten()
        .map(|u| u.username)
        .unwrap_or_default();
    let jwt = match sign_internal_jwt(state, &binding.user_id, &username) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("weixin: sign jwt failed: {e:#}");
            reply("内部错误，请稍后重试").await;
            return;
        }
    };

    let opts = ChatTurnOpts {
        provider: None,
        model: None,
        parent_id: None,
        apply_change_id: None,
        auth_token: jwt,
        extra_prompt_segments: Vec::new(),
    };
    let content = vec![hank_provider::ContentBlock::Text {
        text: format!("{text}{WEIXIN_FILE_HINT}"),
    }];
    let turn = run_chat_turn(state, &session_id, content, opts).await;
    // 到这里 active_tasks 已登记（或启动失败），派发名额可以还了。
    dispatch_guard.release().await;
    match turn {
        Ok(handle) => {
            pusher::spawn(
                state.clone(),
                account.clone(),
                from.to_string(),
                context_token.to_string(),
                session_id.clone(),
                handle.event_rx,
            );
        }
        Err(e) => {
            tracing::warn!("weixin: run_chat_turn failed: {e}");
            state.tasks.clear_progress(&session_id).await;
            let msg = match &e {
                crate::chat::ChatTurnError::UserFacing(m) => m.clone(),
                _ => format!("启动失败：{e}"),
            };
            reply(&msg).await;
        }
    }
}

/// 任务在跑时的回复：带上 pusher 写入的真实进度快照。
async fn running_reply(state: &Arc<AppState>, session_id: &str) -> String {
    match state.tasks.progress(session_id).await {
        Some(snapshot) => format!(
            "任务仍在执行中（{}%）\n当前：{}\n已用时：{}\n完成后会自动汇报；/stop 可取消",
            snapshot.percent,
            snapshot.detail,
            crate::task_state::format_elapsed(snapshot.elapsed())
        ),
        None => "任务刚开始执行，还没有进度产出；完成后会自动汇报，/stop 可取消".to_string(),
    }
}

async fn create_weixin_session(
    state: &Arc<AppState>,
    binding: &WeixinBinding,
) -> Result<hank_db::Session> {
    let metadata = serde_json::json!({ "source": "weixin" }).to_string();
    let session = state
        .db
        .create_session(
            "",
            "",
            None,
            Some(&binding.user_id),
            Some("remote"),
            Some("chat"),
            Some(&metadata),
        )
        .await
        .map_err(|e| anyhow!("create session: {e:#}"))?;
    Ok(session)
}

/// 创建新会话并绑定到当前微信聊天。
async fn new_weixin_session(state: &Arc<AppState>, binding: &WeixinBinding) -> Result<()> {
    let session = create_weixin_session(state, binding).await?;
    state
        .db
        .set_weixin_chat(&binding.id, &session.id)
        .await
        .map_err(|e| anyhow!("set chat: {e:#}"))
}

/// 停止当前映射会话的执行中任务，返回是否有任务被取消。
async fn stop_current_task(state: &Arc<AppState>, binding: &WeixinBinding) -> bool {
    let session_id = state.db.get_weixin_chat(&binding.id).await.ok().flatten();
    match session_id {
        Some(sid) => {
            let tasks = state.active_tasks.read().await;
            tasks.get(&sid).map(|t| t.cancel()).is_some()
        }
        None => false,
    }
}

/// 绑定码有效期（供 routes 生成绑定码时使用）
pub fn bind_code_expires_at() -> i64 {
    chrono::Utc::now().timestamp_millis() + BIND_CODE_TTL_MS
}

#[cfg(test)]
mod tests {
    use super::parse_bind_code;

    #[test]
    fn parses_only_six_digit_bind_codes() {
        assert_eq!(parse_bind_code("bind 766750"), Some("766750"));
        assert_eq!(parse_bind_code("  bind 000001  "), Some("000001"));
        assert_eq!(parse_bind_code("bind 12345"), None);
        assert_eq!(parse_bind_code("bind 1234567"), None);
        assert_eq!(parse_bind_code("bind abcdef"), None);
        assert_eq!(parse_bind_code("binding 123456"), None);
    }
}
