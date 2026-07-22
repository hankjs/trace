//! 微信消息路由：绑定检查 / bind 码 / 斜杠命令 / 渠道 agent 决策分发。

use crate::auth::Claims;
use crate::chat::{run_chat_turn, ChatTurnOpts};
use crate::weixin::api::{IlinkClient, IlinkMessage};
use crate::weixin::channel::{self, ChannelAction};
use crate::weixin::pusher;
use crate::AppState;
use anyhow::{anyhow, Result};
use hank_db::{WeixinAccount, WeixinBinding};
use jsonwebtoken::{encode, EncodingKey, Header};
use std::sync::Arc;

const BIND_CODE_TTL_MS: i64 = 10 * 60 * 1000;

/// 为绑定用户签发短期 JWT（1 小时），供 spec 类工具回调 server 使用。
fn sign_internal_jwt(state: &AppState, user_id: &str, username: &str) -> Result<String> {
    let exp = (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as usize;
    let claims = Claims {
        sub: user_id.to_string(),
        username: username.to_string(),
        can_admin: false,
        can_client: true,
        exp,
    };
    Ok(encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.jwt_secret.as_bytes()),
    )?)
}

pub async fn handle_message(state: Arc<AppState>, account: WeixinAccount, msg: IlinkMessage) -> Result<()> {
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
        .get_weixin_binding(&account.id, &from)
        .await
        .unwrap_or(None);
    if let Some(ref b) = binding {
        if b.context_token.as_deref() != Some(context_token.as_str()) {
            let _ = state.db.update_weixin_binding_context(&b.id, &context_token).await;
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

    // /ai <text>：跳过渠道 agent，直接派发给 coding agent
    if let Some(task) = text.strip_prefix("/ai") {
        let task = task.trim();
        if !task.is_empty() {
            dispatch_task(&state, &account, &binding, &from, &context_token, task, &reply).await;
            return Ok(());
        }
    }

    // 斜杠命令
    if text.starts_with('/') {
        handle_command(&state, &binding, &text, reply).await;
        return Ok(());
    }

    handle_chat(&state, &account, &binding, &from, &context_token, &text, &reply).await;
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
    let code = text
        .strip_prefix("bind")
        .map(str::trim)
        .filter(|c| c.len() == 6 && c.chars().all(|c| c.is_ascii_digit()));
    let code = match code {
        Some(c) => c.to_string(),
        None => {
            reply("请先在 Trace client 生成绑定码，然后发送 bind 123456").await;
            return;
        }
    };
    match state.db.consume_weixin_bind_code(&code).await {
        Ok(Some(user_id)) => {
            match state.db.create_weixin_binding(&account.id, from, &user_id).await {
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

/// 斜杠命令：/new /stop /status。
async fn handle_command<Fut: std::future::Future<Output = ()>>(
    state: &Arc<AppState>,
    binding: &WeixinBinding,
    text: &str,
    reply: impl FnOnce(&str) -> Fut,
) {
    let cmd = text.split_whitespace().next().unwrap_or("");
    let session_id = state.db.get_weixin_chat(&binding.id).await.ok().flatten();
    match cmd {
        "/new" => match new_weixin_session(state, binding).await {
            Ok(()) => reply("已开启新会话").await,
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
                    let title = session.as_ref().map(|s| s.title.clone()).unwrap_or_default();
                    let title = if title.is_empty() { "(无标题)" } else { title.as_str() };
                    // 执行位置：绑定的桌面 client（含在线状态）或 server 本地
                    let exec_at = match session.as_ref().and_then(|s| s.exec_client_id.clone()) {
                        Some(cid) => {
                            let hostname = state
                                .db
                                .get_client_agent(&binding.user_id, &cid)
                                .await
                                .ok()
                                .flatten()
                                .and_then(|c| c.hostname)
                                .unwrap_or_else(|| cid.clone());
                            let online =
                                crate::remote_exec::is_client_online(state, &binding.user_id, &cid)
                                    .await;
                            format!(
                                "client 端 {hostname}{}",
                                if online { "" } else { "（当前离线）" }
                            )
                        }
                        None => "server 端".to_string(),
                    };
                    format!(
                        "会话：{title}\nID：{sid}\n状态：{}\n执行位置：{exec_at}",
                        if running { "执行中" } else { "空闲" }
                    )
                }
                None => "当前没有会话，直接发送消息即可开始".to_string(),
            };
            reply(&msg).await;
        }
        "/terms" => {
            let msg = terminal_list(state, binding).await;
            reply(&msg).await;
        }
        "/term" => {
            let msg = terminal_read(state, binding, text).await;
            reply(&msg).await;
        }
        "/send" => {
            let msg = terminal_write(state, binding, text).await;
            reply(&msg).await;
        }
        "/菜单" | "/menu" | "/help" | "/帮助" => reply(MENU_TEXT).await,
        _ => reply("未知命令，发送 /菜单 查看全部能力").await,
    }
}

// ─── 终端远程命令（/terms /term /send）─────────────────────────────────────

const TERM_CMD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// 微信单条消息长度上限（留余量）
const WECHAT_MSG_MAX: usize = 1800;

/// 向当前在线 client 派发一条 terminal_* 工具调用，返回文本结果或错误文案。
async fn dispatch_terminal(
    state: &Arc<AppState>,
    binding: &WeixinBinding,
    tool: &str,
    input: serde_json::Value,
) -> Result<String> {
    let client = crate::remote_exec::pick_online_client(state, &binding.user_id)
        .await
        .ok_or_else(|| anyhow!("桌面 client 不在线或未开启远程执行"))?;
    let result =
        crate::remote_exec::dispatch_tool_call(state, &binding.user_id, &client.id, tool, input, TERM_CMD_TIMEOUT)
            .await?;
    if result.is_error {
        Err(anyhow!(result.content))
    } else {
        Ok(result.content)
    }
}

/// /terms — 列出 client 上全部终端会话
async fn terminal_list(state: &Arc<AppState>, binding: &WeixinBinding) -> String {
    match dispatch_terminal(state, binding, "terminal_list", serde_json::json!({})).await {
        Ok(content) => {
            let terms: Vec<serde_json::Value> = match serde_json::from_str(&content) {
                Ok(v) => v,
                Err(_) => return format!("解析终端列表失败：{content}"),
            };
            if terms.is_empty() {
                return "当前没有终端会话，在 client 的「终端」页新建一个即可".to_string();
            }
            let mut lines = vec!["终端会话（/term <id> 看输出，/send <id> <文本> 发输入）：".to_string()];
            for t in terms {
                let id = t["id"].as_str().unwrap_or("?");
                let short = &id[..id.len().min(8)];
                let fg = t["foreground_cmd"].as_str().unwrap_or("?");
                let cwd = t["cwd"].as_str().unwrap_or("");
                let alive = if t["alive"].as_bool().unwrap_or(false) { "运行中" } else { "已退出" };
                lines.push(format!("[{short}] {fg} · {alive}\n    {cwd}"));
            }
            lines.join("\n")
        }
        Err(e) => format!("获取终端列表失败：{e:#}"),
    }
}

/// 按 id 前缀解析完整 term_id（前缀需唯一）
async fn resolve_term_id(state: &Arc<AppState>, binding: &WeixinBinding, prefix: &str) -> Result<String> {
    let content = dispatch_terminal(state, binding, "terminal_list", serde_json::json!({})).await?;
    let terms: Vec<serde_json::Value> = serde_json::from_str(&content)?;
    let matches: Vec<&str> = terms
        .iter()
        .filter_map(|t| t["id"].as_str())
        .filter(|id| id.starts_with(prefix))
        .collect();
    match matches.len() {
        0 => Err(anyhow!("没有找到 id 以 {prefix} 开头的终端会话")),
        1 => Ok(matches[0].to_string()),
        _ => Err(anyhow!("id 前缀 {prefix} 匹配到多个会话，请多输几位")),
    }
}

/// 保留字符串尾部不超过 max 个字符（按 char 边界）
fn tail_chars(s: &str, max: usize) -> &str {
    if s.chars().count() <= max {
        return s;
    }
    let start = s.char_indices().nth(s.chars().count() - max).map(|(i, _)| i).unwrap_or(0);
    &s[start..]
}

/// /term <id前缀> [行数] — 查看终端输出尾部
async fn terminal_read(state: &Arc<AppState>, binding: &WeixinBinding, text: &str) -> String {
    let mut parts = text.split_whitespace();
    let _ = parts.next(); // "/term"
    let Some(prefix) = parts.next() else {
        return "用法：/term <id> [行数]，id 用 /terms 查看".to_string();
    };
    let lines: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(80);
    match resolve_term_id(state, binding, prefix).await {
        Ok(id) => {
            match dispatch_terminal(
                state,
                binding,
                "terminal_read",
                serde_json::json!({ "id": id, "lines": lines }),
            )
            .await
            {
                Ok(content) => {
                    let tail = tail_chars(content.trim_end(), WECHAT_MSG_MAX);
                    if tail.is_empty() {
                        "（该终端暂无输出）".to_string()
                    } else {
                        tail.to_string()
                    }
                }
                Err(e) => format!("读取终端输出失败：{e:#}"),
            }
        }
        Err(e) => format!("{e:#}"),
    }
}

/// /send <id前缀> <文本> — 向终端发送输入（末尾自动补回车）
async fn terminal_write(state: &Arc<AppState>, binding: &WeixinBinding, text: &str) -> String {
    let mut parts = text.splitn(3, ' ');
    let _ = parts.next(); // "/send"
    let Some(prefix) = parts.next().map(str::trim).filter(|s| !s.is_empty()) else {
        return "用法：/send <id> <文本>，id 用 /terms 查看".to_string();
    };
    let Some(data) = parts.next() else {
        return "用法：/send <id> <文本>，文本不能为空".to_string();
    };
    match resolve_term_id(state, binding, prefix).await {
        Ok(id) => {
            match dispatch_terminal(
                state,
                binding,
                "terminal_write",
                serde_json::json!({ "id": id, "data": format!("{data}\r") }),
            )
            .await
            {
                Ok(_) => format!("已发送到 [{prefix}]，稍后用 /term {prefix} 查看输出"),
                Err(e) => format!("发送失败：{e:#}"),
            }
        }
        Err(e) => format!("{e:#}"),
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

【终端（桌面 client 在线时可用）】
/terms — 列出 client 上的终端会话
/term <id> [行数] — 查看终端输出（如 /term a1b2 100）
/send <id> <文本> — 向终端发送输入（如 /send a1b2 ls -la）

【小贴士】
· 桌面 client 在线时任务在你本地机器执行，离线时在服务器端执行、仅支持查询类对话
· 同一时间只能跑一个任务，执行中新消息会排队提醒
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
    match channel::decide(state, binding, session_id.as_deref(), text).await {
        Some(ChannelAction::Reply { text: t }) => reply(&t).await,
        Some(ChannelAction::Dispatch { ack, task }) => {
            if !ack.trim().is_empty() {
                reply(&ack).await;
            }
            let task = if task.trim().is_empty() { text } else { task.trim() };
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
            reply(&msg).await;
        }
        Some(ChannelAction::New { text: t }) => match new_weixin_session(state, binding).await {
            Ok(()) => {
                let msg = if t.trim().is_empty() { "已开启新会话" } else { t.trim() };
                reply(msg).await;
            }
            Err(e) => {
                tracing::warn!("weixin: create session failed: {e:#}");
                reply("创建会话失败，请稍后重试").await;
            }
        },
        None => dispatch_task(state, account, binding, from, context_token, text, reply).await,
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

    // 并发控制：同 session 同时只跑一个 turn
    if state.active_tasks.read().await.contains_key(&session_id) {
        reply("正在执行中，/stop 可取消").await;
        return;
    }

    // 会话绑定的桌面 client 已离线时解除绑定，退化为 server 本地执行
    if let Ok(Some(session)) = state.db.get_session(&session_id).await {
        if let Some(ref cid) = session.exec_client_id {
            if !crate::remote_exec::is_client_online(state, &binding.user_id, cid).await {
                tracing::info!(session_id = %session_id, client_id = %cid, "exec client offline, unbind session");
                let _ = state.db.set_session_exec_client(&session_id, None, None).await;
            }
        }
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
    };
    let content = vec![hank_provider::ContentBlock::Text {
        text: format!("{text}{WEIXIN_FILE_HINT}"),
    }];
    match run_chat_turn(state, &session_id, content, opts).await {
        Ok(handle) => {
            pusher::spawn(
                state.clone(),
                account.clone(),
                from.to_string(),
                context_token.to_string(),
                session_id.clone(),
                binding.user_id.clone(),
                handle.event_rx,
            );
        }
        Err(e) => {
            tracing::warn!("weixin: run_chat_turn failed: {e}");
            reply(&format!("启动失败：{e}")).await;
        }
    }
}

async fn create_weixin_session(state: &Arc<AppState>, binding: &WeixinBinding) -> Result<hank_db::Session> {
    let metadata = serde_json::json!({ "source": "weixin" }).to_string();
    // 有在线且接受远程任务的桌面 client 时，会话绑定到该 client 本地执行
    let client = crate::remote_exec::pick_online_client(state, &binding.user_id).await;
    let work_dir = client.as_ref().and_then(|c| c.work_dir.clone());
    let mut session = state
        .db
        .create_session(
            "",
            "",
            work_dir.as_deref(),
            Some(&binding.user_id),
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
            tracing::warn!("weixin: bind exec client failed: {e:#}");
        } else {
            tracing::info!(session_id = %session.id, client_id = %c.id, "weixin session bound to desktop client");
            session.exec_client_id = Some(c.id);
            session.work_dir = c.work_dir;
        }
    }
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
