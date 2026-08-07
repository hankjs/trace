//! handy 入站路由：话题留言 → trace 会话 → run_chat_turn 派发。
//!
//! 入站由 handy 的 `message.created` webhook 主动推送（webhook.rs 验签、按
//! message_id 幂等后 spawn 到这里）。与 weixin router 的差异：handy 没有
//! 绑定/命令/渠道 agent 那套前置路由，用户消息直接派发给映射会话；话题
//! 没有映射时自动建会话登记（handy 网页徒手开的新话题也能发起任务）。
//! pending ask_user 的文字作答由 chat.rs 的 resolve_pending_ask_user
//! 零改动接管（run_chat_turn 开头统一解析，与渠道无关）；
//! pusher 由 run_chat_turn 按 metadata.source=="handy" 自动挂。

use crate::chat::{run_chat_turn, ChatTurnOpts};
use crate::handy::client::{HandyApi, HandyMessage};
use crate::AppState;
use anyhow::{anyhow, Result};
use std::sync::Arc;

/// 会话标题用的留言截断长度（字符）
const SESSION_TITLE_CHARS: usize = 30;

/// 处理一条 handy 话题里的用户消息。user_id 是 webhook 路径解析出的
/// handy 账号属主（会话归属与内部 JWT 都以它为准）。
///
/// 失败只记日志（webhook 路径下 handy 已按 2xx 标已读，没有重推机会），
/// 所以这里尽量把失败转成给用户的一条 handy 回复，而不是静默吞掉。
pub async fn handle_user_message(
    state: &Arc<AppState>,
    api: &HandyApi,
    user_id: &str,
    topic_id: &str,
    msg: &HandyMessage,
) -> Result<()> {
    let text = msg.content.trim();
    if text.is_empty() {
        return Ok(());
    }

    let session_id = ensure_session(state, user_id, topic_id, text).await?;

    // 并发控制：同 session 同时只跑一个 turn（与 weixin/feishu 同口径）。
    // 忙时回一条提示后消费掉这条消息。
    let Some(dispatch_guard) = state.tasks.try_acquire(&session_id).await else {
        reply(api, topic_id, &running_reply(state, &session_id).await).await;
        return Ok(());
    };
    if state.active_tasks.read().await.contains_key(&session_id) {
        dispatch_guard.release().await;
        reply(api, topic_id, &running_reply(state, &session_id).await).await;
        return Ok(());
    }

    let opts = ChatTurnOpts {
        provider: None,
        model: None,
        parent_id: None,
        apply_change_id: None,
        auth_token: sign_handy_jwt(state, user_id).await,
        extra_prompt_segments: Vec::new(),
    };
    let content = vec![hank_provider::ContentBlock::Text {
        text: text.to_string(),
    }];
    let turn = run_chat_turn(state, &session_id, content, opts).await;
    // 到这里 active_tasks 已登记（或启动失败），派发名额可以还了。
    dispatch_guard.release().await;
    match turn {
        Ok(_handle) => {
            // pusher 已由 run_chat_turn 自动挂接（metadata.source=="handy"），
            // 事件流不需要这里再消费。
        }
        Err(e) => {
            tracing::warn!("handy: run_chat_turn failed: {e}");
            state.tasks.clear_progress(&session_id).await;
            let msg = match &e {
                crate::chat::ChatTurnError::UserFacing(m) => m.clone(),
                _ => format!("启动失败：{e}"),
            };
            reply(api, topic_id, &msg).await;
        }
    }
    Ok(())
}

/// 取话题映射的 session_id：没有映射（handy 网页徒手开的新话题）或映射的
/// 会话已被删除时，建会话（metadata {"source":"handy"}）并登记映射。
/// 新建会话的标题用首条留言截断（handy 推送的 payload 里没有话题标题）。
async fn ensure_session(
    state: &Arc<AppState>,
    user_id: &str,
    topic_id: &str,
    first_text: &str,
) -> Result<String> {
    if let Some(chat) = state.db.get_handy_chat(topic_id).await? {
        if let Some(session) = state.db.get_session(&chat.session_id).await.ok().flatten() {
            // 映射被另一用户的 handy 实例撞 id 抢走时，会话不属于当前账号属主，
            // 不往里派消息（话题留言会落到别人会话里），按无映射重建。
            if session.user_id.as_deref() == Some(user_id) {
                return Ok(chat.session_id);
            }
            tracing::warn!(topic_id, "handy 映射会话归属与账号属主不符，重建会话");
        } else {
            tracing::info!(topic_id, "handy 映射会话已删除，重建会话");
        }
    }
    let session = create_handy_session(state, user_id).await?;
    let title = session_title_from(first_text);
    if !title.is_empty() {
        if let Err(e) = state.db.update_session_title(&session.id, &title).await {
            tracing::warn!("handy: 写会话标题失败: {e:#}");
        }
    }
    state.db.set_handy_chat(topic_id, &session.id).await?;
    tracing::info!(topic_id, session_id = %session.id, "handy 新话题已建会话并登记映射");
    Ok(session.id)
}

/// 会话标题：首条留言的第一行，截断到 SESSION_TITLE_CHARS 字符。
fn session_title_from(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(SESSION_TITLE_CHARS)
        .collect()
}

/// 建 handy 渠道会话：归属账号属主，metadata 写 {"source":"handy"}
/// （照 weixin/router.rs 的模式；run_chat_turn 靠 source 自动挂 pusher）。
pub async fn create_handy_session(state: &Arc<AppState>, user_id: &str) -> Result<hank_db::Session> {
    let metadata = serde_json::json!({ "source": "handy" }).to_string();
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

/// 为 handy 账号属主签内部 JWT（spec 类工具回调 server 用）。
/// 用户被删时返回空串：会话照常跑，spec 类工具回调会 401。
async fn sign_handy_jwt(state: &Arc<AppState>, user_id: &str) -> String {
    let username = state
        .db
        .get_user_by_id(user_id)
        .await
        .ok()
        .flatten()
        .map(|u| u.username)
        .unwrap_or_default();
    if username.is_empty() {
        return String::new();
    }
    crate::auth::sign_internal_jwt(&state.jwt_secret, user_id, &username).unwrap_or_default()
}

/// 任务在跑时的回复：带上 pusher 写入的真实进度快照（照 weixin running_reply）。
async fn running_reply(state: &Arc<AppState>, session_id: &str) -> String {
    match state.tasks.progress(session_id).await {
        Some(snapshot) => format!(
            "任务仍在执行中（{}%）\n当前：{}\n已用时：{}\n完成后卡片会自动更新",
            snapshot.percent,
            snapshot.detail,
            crate::task_state::format_elapsed(snapshot.elapsed())
        ),
        None => "任务刚开始执行，还没有进度产出；完成后卡片会自动更新".to_string(),
    }
}

/// 回一条 assistant 消息到 handy 话题；失败只记日志（消息已被消费）。
async fn reply(api: &HandyApi, topic_id: &str, text: &str) {
    if let Err(e) = api.post_message(topic_id, text).await {
        tracing::warn!(topic_id, "handy: reply failed: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_title_truncates_first_line() {
        assert_eq!(session_title_from("帮我查一下库存"), "帮我查一下库存");
        // 多行只取第一行
        assert_eq!(session_title_from("第一行\n第二行"), "第一行");
        // 超长截断（按字符不是字节）
        let long: String = "字".repeat(50);
        assert_eq!(session_title_from(&long).chars().count(), SESSION_TITLE_CHARS);
        assert_eq!(session_title_from(""), "");
    }
}
