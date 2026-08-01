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
    Ok(crate::auth::sign_internal_jwt(&state.jwt_secret, user_id, username)?)
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
        .get_weixin_binding_by_bot(&account.ilink_bot_id, &from)
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

    // bind 是一次性绑定流程。重复投递或用户再次发送绑定码都必须停在
    // 渠道层，不能因为“已经绑定”而把原始 bind 文本派发给 coding agent。
    if parse_bind_code(&text).is_some() {
        reply("这个微信已经绑定 Trace 账号，直接发消息即可开始对话").await;
        return Ok(());
    }

    // kimi <text>：转发给托管的 Kimi CLI 终端（含 kimi /yolo 这类 CLI 自身命令）
    // 必须放在 /ai 与斜杠命令检查之前，避免内容以 / 开头时被命令分支拦截
    if let Some(input) = text.strip_prefix("kimi ").or_else(|| text.strip_prefix("Kimi ")) {
        let input = input.trim();
        if !input.is_empty() {
            if let Some(msg) = crate::weixin::kimi::send_input(&state, &binding, input).await {
                reply(&msg).await;
            }
            return Ok(());
        }
    }

    // /ai <text>：跳过渠道 agent，直接派发给 coding agent
    if let Some(task) = text.strip_prefix("/ai") {
        let task = task.trim();
        if !task.is_empty() {
            channel::push_history(&state, &binding.id, task, "收到，任务已派发").await;
            dispatch_task(&state, &account, &binding, &from, &context_token, task, &reply).await;
            return Ok(());
        }
    }

    // 不带斜杠的菜单请求：直接回固定文案，避免渠道 agent 凭印象编功能清单
    let trimmed = text.trim();
    if trimmed == "菜单" || trimmed == "帮助" || trimmed.eq_ignore_ascii_case("menu") || trimmed.eq_ignore_ascii_case("help") {
        reply(MENU_TEXT).await;
        return Ok(());
    }

    // 斜杠命令
    if text.starts_with('/') {
        handle_command(&state, &account, &binding, &from, &context_token, &text, reply).await;
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
    let code = match parse_bind_code(text) {
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
            let mut msg = match &session_id {
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
                    let mut text = format!(
                        "会话：{title}\nID：{sid}\n状态：{}\n执行位置：{exec_at}",
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
            if let Some(line) = crate::weixin::kimi::status_line(state, binding).await {
                msg.push_str("\n");
                msg.push_str(&line);
            }
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
        "/shot" => {
            let msg = terminal_shot(state, account, binding, from, context_token, text).await;
            reply(&msg).await;
        }
        "/snap" => {
            let msg = web_snap(state, account, from, context_token, text).await;
            reply(&msg).await;
        }
        "/cd" => {
            let dir = text.splitn(2, ' ').nth(1).map(str::trim).unwrap_or("");
            if dir.is_empty() {
                let current = state
                    .db
                    .get_weixin_kimi(&binding.id)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|m| m.work_dir);
                match current {
                    Some(d) => {
                        reply(&format!("当前工作目录：{d}\n/cd <路径> 修改，/ls [路径] 浏览目录")).await
                    }
                    None => reply("还没有设置工作目录，/ls [路径] 先看看有哪些目录").await,
                }
            } else {
                // 在 client 上真实校验目录存在，并解析为绝对路径（支持 ~）
                match client_shell(state, binding, &format!("cd {} && pwd", sh_quote(dir))).await {
                    Ok(out) => {
                        let resolved = out
                            .lines()
                            .last()
                            .map(str::trim)
                            .filter(|s| s.starts_with('/'))
                            .unwrap_or(dir);
                        match state.db.upsert_weixin_kimi_work_dir(&binding.id, resolved).await {
                            Ok(()) => {
                                // 已有存活托管会话时提示不即时生效
                                let note = match crate::weixin::kimi::status_line(state, binding).await {
                                    Some(line) if line.contains("运行中") => {
                                        "\n（当前 kimi 会话不受影响，/kstop 后重开才会切换）"
                                    }
                                    _ => "",
                                };
                                reply(&format!("已记录工作目录：{resolved}{note}")).await;
                            }
                            Err(e) => {
                                tracing::warn!("weixin: set kimi work dir failed: {e:#}");
                                reply("设置失败，请稍后重试").await;
                            }
                        }
                    }
                    Err(e) => {
                        let msg = format!("{e:#}");
                        if msg.contains("不在线") {
                            reply(&msg).await;
                        } else {
                            reply(&format!(
                                "目录不存在或不可进入：{dir}\n用 /ls [路径] 看看 client 上有哪些目录"
                            ))
                            .await;
                        }
                    }
                }
            }
        }
        "/ls" => {
            let msg = kimi_list_dir(state, binding, text).await;
            reply(&msg).await;
        }
        "/kimi" => match crate::weixin::kimi::start_session(state, binding).await {
            Ok(msg) => reply(&msg).await,
            Err(e) => {
                tracing::warn!("weixin: start kimi session failed: {e:#}");
                reply(&format!("开启失败：{e:#}")).await;
            }
        },
        "/kstop" => {
            let msg = crate::weixin::kimi::stop_session(state, binding).await;
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

/// shell 路径转义：单引号包裹，~ 前缀展开为 $HOME
fn sh_quote(path: &str) -> String {
    if path == "~" {
        "$HOME".to_string()
    } else if let Some(rest) = path.strip_prefix("~/") {
        format!("$HOME/'{}'", rest.replace('\'', "'\\''"))
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

/// 在在线 client 上执行一条 shell 命令（/ls、/cd 校验用）
async fn client_shell(state: &Arc<AppState>, binding: &WeixinBinding, command: &str) -> Result<String> {
    dispatch_terminal(
        state,
        binding,
        "shell",
        serde_json::json!({ "command": command, "timeout_ms": 10000 }),
    )
    .await
}

/// /ls [路径] — 列出 client 上的目录内容，方便挑选 /cd 目标。
/// 默认路径：/cd 已记录的目录 → client 注册的工作目录 → home。
async fn kimi_list_dir(state: &Arc<AppState>, binding: &WeixinBinding, text: &str) -> String {
    let arg = text.splitn(2, ' ').nth(1).map(str::trim).filter(|s| !s.is_empty());
    let path = match arg {
        Some(p) => p.to_string(),
        None => {
            let recorded = state
                .db
                .get_weixin_kimi(&binding.id)
                .await
                .ok()
                .flatten()
                .and_then(|m| m.work_dir);
            match recorded {
                Some(d) => d,
                None => crate::remote_exec::pick_online_client(state, &binding.user_id)
                    .await
                    .and_then(|c| c.work_dir)
                    .unwrap_or_else(|| "~".to_string()),
            }
        }
    };
    match client_shell(state, binding, &format!("ls -la {}", sh_quote(&path))).await {
        Ok(out) => {
            let out = out.trim_end();
            if out.is_empty() {
                return format!("{path}：空目录");
            }
            if out.chars().count() > WECHAT_MSG_MAX {
                let head: String = out.chars().take(WECHAT_MSG_MAX - 40).collect();
                format!("{path}：\n{head}\n…（过长已截断，/ls <子目录> 细化）")
            } else {
                format!("{path}：\n{out}")
            }
        }
        Err(e) => format!("ls 失败：{e:#}"),
    }
}

/// /terms — 列出 client 上全部终端会话
async fn terminal_list(state: &Arc<AppState>, binding: &WeixinBinding) -> String {    match dispatch_terminal(state, binding, "terminal_list", serde_json::json!({})).await {
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
/// /shot <id前缀> — 终端屏幕截图：拉 SGR 快照渲染成 PNG 发图，失败降级为文本输出
async fn terminal_shot(
    state: &Arc<AppState>,
    account: &WeixinAccount,
    binding: &WeixinBinding,
    from: &str,
    context_token: &str,
    text: &str,
) -> String {
    let mut parts = text.split_whitespace();
    let _ = parts.next(); // "/shot"
    let Some(prefix) = parts.next() else {
        return "用法：/shot <id>，id 用 /terms 查看".to_string();
    };
    let id = match resolve_term_id(state, binding, prefix).await {
        Ok(id) => id,
        Err(e) => return format!("{e:#}"),
    };
    // raw=true：client 返回带 SGR 转义码的当前屏幕快照
    let snap = match dispatch_terminal(
        state,
        binding,
        "terminal_read",
        serde_json::json!({ "id": id, "raw": true }),
    )
    .await
    {
        Ok(content) => content,
        Err(e) => return format!("读取终端屏幕失败：{e:#}"),
    };
    let fallback = |reason: &str| {
        // 渲染/发图失败时降级为纯文本输出（复用 /term 的截尾逻辑）
        let plain = crate::termshot::strip_ansi(&snap);
        let tail = tail_chars(plain.trim_end(), WECHAT_MSG_MAX);
        if tail.is_empty() {
            reason.to_string()
        } else {
            format!("{reason}，以下为终端文本：\n{tail}")
        }
    };
    let png = match crate::termshot::render_png(&snap) {
        Ok(png) => png,
        Err(e) => {
            tracing::warn!("weixin: render term shot failed: {e:#}");
            return fallback(&format!("截图渲染失败：{e:#}"));
        }
    };
    let client = IlinkClient::new();
    match client.send_media(account, from, context_token, "term.png", &png).await {
        Ok(()) => format!("终端 [{prefix}] 截图已发送"),
        Err(e) => {
            tracing::warn!("weixin: send term shot failed: {e:#}");
            fallback(&format!("截图发送失败：{e:#}"))
        }
    }
}

/// /snap <url> — 网页截图：server 本机 headless Chrome 截全页 PNG 发图。
/// 耗时可达 30s；monitor 对每条消息单独 spawn（monitor.rs），这里直接 await 不会阻塞轮询。
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
    if let Err(e) = client.send_text(account, from, context_token, "网页截图中，请稍候…").await {
        tracing::warn!("weixin: snap ack reply failed: {e:#}");
    }
    let png = match crate::websnap::snap_url(state.config.server.chrome_path.as_deref(), url).await {
        Ok(png) => png,
        Err(e) => return format!("网页截图失败：{e:#}"),
    };
    match client.send_media(account, from, context_token, "snap.png", &png).await {
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

【终端（桌面 client 在线时可用）】
/terms — 列出 client 上的终端会话
/term <id> [行数] — 查看终端输出（如 /term a1b2 100）
/send <id> <文本> — 向终端发送输入（如 /send a1b2 ls -la）
/shot <id> — 终端屏幕截图（图片）
/snap <url> — 网页截图（图片）

【kimi 托管（桌面 client 在线时可用）】
/ls [路径] — 浏览 client 上的目录（挑 /cd 目标）
/cd <路径> — 设置托管终端的工作目录（会在 client 上校验）
/kimi — 在 client 上开一个托管的 Kimi CLI 会话
kimi <文本> — 与托管 kimi 交互（如 kimi /yolo、kimi y）
· 运行期间不推送进度；需要决策或完成任务时才通知你
/kstop — 结束托管会话

【小贴士】
· 桌面 client 在线时任务在你本地机器执行，离线时在服务器端执行、仅支持查询类对话
· 同一时间只能跑一个任务，执行中新消息会排队提醒
· 也可以直接说「截图 xxx」（网页或终端），agent 会自动截好发你
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
            let ack_text = if ack.trim().is_empty() { "收到，任务已派发" } else { ack.trim() };
            channel::push_history(state, &binding.id, text, ack_text).await;
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
            channel::push_history(state, &binding.id, text, &msg).await;
            reply(&msg).await;
        }
        Some(ChannelAction::New { text: t }) => match new_weixin_session(state, binding).await {
            Ok(()) => {
                channel::clear_history(state, &binding.id).await;
                let msg = if t.trim().is_empty() { "已开启新会话" } else { t.trim() };
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
                binding.user_id.clone(),
                handle.event_rx,
            );
        }
        Err(e) => {
            tracing::warn!("weixin: run_chat_turn failed: {e}");
            state.tasks.clear_progress(&session_id).await;
            reply(&format!("启动失败：{e}")).await;
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
