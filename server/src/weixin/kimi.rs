//! Kimi CLI 托管会话：在桌面 client 上 spawn 终端运行 kimi，微信远程喂输入。
//!
//! 托管期间微信侧无感知（不接 pusher、无进度推送）；kimi 的 OSC 通知由 client
//! 捕获上报到 `client_notifications` 表，本模块的消费循环把属于托管终端的通知
//! 推回微信。用户用 `kimi <文本>` 前缀消息应答/追问（router 里路由到 send_input）。

use crate::weixin::api::IlinkClient;
use crate::AppState;
use anyhow::{anyhow, Result};
use hank_db::WeixinBinding;
use std::sync::Arc;
use std::time::Duration;

const TERM_CMD_TIMEOUT: Duration = Duration::from_secs(15);
/// 通知消费循环间隔
const CONSUMER_TICK: Duration = Duration::from_secs(3);

/// 向托管会话所属的指定 client 派发一条 terminal_* 工具调用。
/// 不能用 router::dispatch_terminal（它走 pick_online_client，多台 client 时可能选错机器）。
async fn dispatch_to(
    state: &Arc<AppState>,
    user_id: &str,
    client_id: &str,
    tool: &str,
    input: serde_json::Value,
) -> Result<String> {
    let result =
        crate::remote_exec::dispatch_tool_call(state, user_id, client_id, tool, input, TERM_CMD_TIMEOUT)
            .await?;
    if result.is_error {
        Err(anyhow!(result.content))
    } else {
        Ok(result.content)
    }
}

/// 查询托管终端是否仍存活，存活时返回终端列表中的条目
async fn alive_term(
    state: &Arc<AppState>,
    user_id: &str,
    client_id: &str,
    term_id: &str,
) -> Option<serde_json::Value> {
    let content = dispatch_to(state, user_id, client_id, "terminal_list", serde_json::json!({}))
        .await
        .ok()?;
    let terms: Vec<serde_json::Value> = serde_json::from_str(&content).ok()?;
    terms.into_iter().find(|t| {
        t["id"].as_str() == Some(term_id) && t["alive"].as_bool().unwrap_or(false)
    })
}

/// 托管会话状态描述（/status、/kimi 复用）；无映射时返回 None
pub async fn status_line(state: &Arc<AppState>, binding: &WeixinBinding) -> Option<String> {
    let mapping = state.db.get_weixin_kimi(&binding.id).await.ok().flatten()?;
    let (client_id, term_id) = (mapping.client_id?, mapping.term_id?);
    let short = &term_id[..term_id.len().min(8)];
    let cwd = mapping.work_dir.as_deref().unwrap_or("");
    match alive_term(state, &binding.user_id, &client_id, &term_id).await {
        Some(_) => Some(format!(
            "kimi 托管：运行中（终端 {short} · {cwd}，/term {short} 看输出）"
        )),
        None => Some(format!("kimi 托管：已退出（/kimi 重新开启）")),
    }
}

/// /kimi：已有存活会话则报告状态，否则在在线 client 上开新终端启动 kimi CLI
pub async fn start_session(state: &Arc<AppState>, binding: &WeixinBinding) -> Result<String> {
    let mapping = state.db.get_weixin_kimi(&binding.id).await.ok().flatten();
    let work_dir = mapping.as_ref().and_then(|m| m.work_dir.clone());

    // 已有映射且终端存活：不重复开
    if let Some(ref m) = mapping {
        if let (Some(cid), Some(tid)) = (m.client_id.clone(), m.term_id.clone()) {
            if alive_term(state, &binding.user_id, &cid, &tid).await.is_some() {
                let short = &tid[..tid.len().min(8)];
                return Ok(format!(
                    "kimi 托管会话已在运行（终端 {short}），直接发送 kimi <文本> 即可交互，/kstop 结束"
                ));
            }
        }
    }

    let client = crate::remote_exec::pick_online_client(state, &binding.user_id)
        .await
        .ok_or_else(|| anyhow!("桌面 client 不在线或未开启远程执行"))?;

    let created = dispatch_to(
        state,
        &binding.user_id,
        &client.id,
        "terminal_create",
        serde_json::json!({ "cwd": work_dir }),
    )
    .await?;
    let info: serde_json::Value = serde_json::from_str(&created)
        .map_err(|e| anyhow!("解析 terminal_create 结果失败：{e}"))?;
    let term_id = info["id"]
        .as_str()
        .ok_or_else(|| anyhow!("terminal_create 结果缺少 id"))?
        .to_string();

    // PTY 输入有缓冲，shell 就绪后会读到这行并启动 kimi CLI
    dispatch_to(
        state,
        &binding.user_id,
        &client.id,
        "terminal_write",
        serde_json::json!({ "id": term_id, "data": "kimi\r" }),
    )
    .await?;

    state
        .db
        .set_weixin_kimi(&binding.id, &client.id, &term_id)
        .await?;
    tracing::info!(binding_id = %binding.id, client_id = %client.id, term_id, "weixin kimi session started");

    let short = &term_id[..term_id.len().min(8)];
    let cwd = work_dir.as_deref().unwrap_or("client 默认目录");
    Ok(format!(
        "已开启 kimi 托管会话（终端 {short} · {cwd}）\n\
         发送 kimi <文本> 与它交互（如 kimi /yolo），运行期间不打扰你；\n\
         需要你决策时会推送通知，/status 查看状态，/kstop 结束"
    ))
}

/// `kimi <文本>` 前缀消息：原样写入托管终端（末尾补回车）。
/// 返回 None 表示成功（静默，无感知）；Some 为需要回复给用户的错误/提示文案。
pub async fn send_input(state: &Arc<AppState>, binding: &WeixinBinding, text: &str) -> Option<String> {
    let mapping = match state.db.get_weixin_kimi(&binding.id).await {
        Ok(Some(m)) => m,
        _ => return Some("没有运行中的 kimi 托管会话，发送 /kimi 先开启".to_string()),
    };
    let (client_id, term_id) = match (mapping.client_id, mapping.term_id) {
        (Some(c), Some(t)) => (c, t),
        _ => return Some("没有运行中的 kimi 托管会话，发送 /kimi 先开启".to_string()),
    };
    if alive_term(state, &binding.user_id, &client_id, &term_id).await.is_none() {
        return Some("kimi 托管会话已退出，发送 /kimi 重新开启".to_string());
    }
    match dispatch_to(
        state,
        &binding.user_id,
        &client_id,
        "terminal_write",
        serde_json::json!({ "id": term_id, "data": format!("{text}\r") }),
    )
    .await
    {
        Ok(_) => None,
        Err(e) => {
            tracing::warn!("weixin kimi: write input failed: {e:#}");
            Some(format!("发送失败：{e:#}"))
        }
    }
}

/// /kstop：关闭托管终端并清除映射（work_dir 保留）
pub async fn stop_session(state: &Arc<AppState>, binding: &WeixinBinding) -> String {
    let mapping = match state.db.get_weixin_kimi(&binding.id).await {
        Ok(Some(m)) => m,
        _ => return "当前没有 kimi 托管会话".to_string(),
    };
    if let (Some(client_id), Some(term_id)) = (mapping.client_id, mapping.term_id) {
        let _ = dispatch_to(
            state,
            &binding.user_id,
            &client_id,
            "terminal_close",
            serde_json::json!({ "id": term_id }),
        )
        .await;
    }
    let _ = state.db.clear_weixin_kimi(&binding.id).await;
    "kimi 托管会话已结束".to_string()
}

/// 通知消费循环：把属于托管终端的 client 通知推回微信。
/// 在 main.rs 启动时 spawn；3s 一轮扫 client_notifications 的未消费记录。
pub async fn run_notification_consumer(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(CONSUMER_TICK);
    loop {
        interval.tick().await;
        if let Err(e) = consume_once(&state).await {
            tracing::warn!("weixin kimi: notification consumer tick failed: {e:#}");
        }
    }
}

async fn consume_once(state: &Arc<AppState>) -> Result<()> {
    let pending = state.db.list_unpushed_client_notifications(50).await?;
    for n in pending {
        // 只推送给「该用户微信绑定 + 有托管映射 + term_id 匹配」的通知，其余直接标记已消费
        let pushed = try_push_notification(state, &n).await;
        match pushed {
            Ok(sent) => {
                state.db.mark_client_notification_pushed(&n.id).await?;
                if sent {
                    tracing::info!(notification_id = %n.id, title = %n.title, "weixin kimi: notification pushed");
                }
            }
            Err(e) => {
                // 发送失败不标记，下一轮重试（context_token 失效会持续失败，仅记日志）
                tracing::warn!(notification_id = %n.id, "weixin kimi: push failed, will retry: {e:#}");
            }
        }
    }
    Ok(())
}

/// 返回 Ok(true)=已推送，Ok(false)=不属于托管终端（直接标记消费），Err=发送失败
async fn try_push_notification(
    state: &Arc<AppState>,
    n: &hank_db::ClientNotification,
) -> Result<bool> {
    let binding = match state.db.get_weixin_binding_by_user(&n.user_id).await? {
        Some(b) => b,
        None => return Ok(false),
    };
    let mapping = match state.db.get_weixin_kimi(&binding.id).await? {
        Some(m) => m,
        None => return Ok(false),
    };
    match (&mapping.term_id, &n.term_id) {
        (Some(mapped), Some(reported)) if mapped == reported => {}
        _ => return Ok(false),
    }
    let context_token = match binding.context_token.as_deref() {
        Some(t) if !t.is_empty() => t,
        _ => return Err(anyhow!("binding 没有可用的 context_token")),
    };
    let account = state
        .db
        .get_weixin_account(&binding.account_id)
        .await?
        .ok_or_else(|| anyhow!("weixin account 不存在"))?;
    let body = n.body.as_deref().unwrap_or("").trim();
    let text = if body.is_empty() {
        format!("【kimi】{}\n\n回复 kimi <内容> 继续操作", n.title)
    } else {
        format!("【kimi】{}\n{body}\n\n回复 kimi <内容> 继续操作", n.title)
    };
    IlinkClient::new()
        .send_text(&account, &binding.ilink_user_id, context_token, &text)
        .await?;
    Ok(true)
}
