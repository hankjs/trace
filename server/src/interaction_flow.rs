//! 交互单应答与 resume 派发的共用流程。
//!
//! 为什么飞书回调与 admin 必须共用：两处各写一遍顺序会漂移，
//! 漏掉「先抢名额再应答」或「派发失败回滚」就会让确认被静默吞掉
//! （状态已 answered 但无人消费，且因 WHERE status='pending' 无法重试）。
//!
//! 顺序必须是 **抢名额 → claim（可选）→ 应答 → 改卡片（可选）→ 派发**：
//! 若先应答再抢名额，会留下「answered 但未派发」的不可恢复僵尸。

use crate::chat::{run_chat_turn, ChatTurnOpts};
use crate::feishu::api::FeishuApi;
use crate::feishu::card::{build_confirm_card, build_confirm_done_card, ConfirmCardOptions};
use crate::feishu::pusher;
use crate::task_state::DispatchGuard;
use crate::AppState;
use anyhow::{anyhow, Result};
use hank_db::{AgentInteraction, FeishuAccount};
use serde_json::Value;
use std::sync::Arc;

/// 飞书卡片回调上下文。admin 手动应答传 None，跳过 claim / 改卡 / 恢复卡。
pub struct ChannelCardContext {
    pub api: FeishuApi,
    pub account: FeishuAccount,
    pub card_message_id: Option<String>,
    pub event_id: Option<String>,
    pub operator_open_id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// 卡片 payload 上的 chat_id（与交互单上的对照，优先用交互单）
    pub chat_id: String,
    pub topic_id: String,
    /// 卡片 payload 上的 question 兜底
    pub question_fallback: Option<String>,
}

/// 用户可读失败原因（名额被占、已被抢答、已过期等）。
#[derive(Debug)]
pub struct AnswerResumeError {
    pub message: String,
}

impl AnswerResumeError {
    fn new(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
        }
    }
}

impl std::fmt::Display for AnswerResumeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// resume 入参打包，避免 too_many_arguments。
struct ResumeInteraction {
    state: Arc<AppState>,
    /// 有则在成功时推飞书进度卡；失败时按 restore_card 决定是否恢复确认卡。
    api: Option<FeishuApi>,
    user_id: String,
    session_id: String,
    interaction_id: String,
    text: String,
    message_id: String,
    chat_id: String,
    topic_id: String,
    in_thread: bool,
    card_message_id: Option<String>,
    /// admin 路径不改卡片，失败时也不恢复卡。
    restore_card_on_fail: bool,
    dispatch_guard: DispatchGuard,
}

/// 应答一张交互单并派发 resume：抢名额 → 原子应答 → 派发 → 失败回滚。
/// 飞书按钮回调与 admin 手动应答共用，避免两处各写一遍顺序而漂移。
///
/// 返回 Err 表示未能应答（名额被占、已被抢答、已过期），调用方转成用户可读提示。
pub async fn answer_and_resume(
    state: &Arc<AppState>,
    interaction_id: &str,
    answer: &str,
    operator_user_id: &str,
    channel_ctx: Option<ChannelCardContext>,
) -> Result<(), AnswerResumeError> {
    if interaction_id.is_empty() {
        return Err(AnswerResumeError::new("交互单 id 为空"));
    }

    let interaction_row = state
        .db
        .get_interaction(interaction_id)
        .await
        .map_err(|e| AnswerResumeError::new(format!("读取交互单失败: {e:#}")))?;
    let Some(interaction_row) = interaction_row else {
        return Err(AnswerResumeError::new("交互单不存在或已失效"));
    };

    // 派发必须落到交互单冻结的 session_id，而不是渠道当前话题映射。
    let session_id = interaction_row.session_id.clone();
    if session_id.is_empty() {
        return Err(AnswerResumeError::new("交互单缺少 session_id，无法派发"));
    }

    let chat_id = interaction_row
        .chat_id
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| channel_ctx.as_ref().map(|c| c.chat_id.clone()))
        .unwrap_or_default();
    let topic_id = interaction_row
        .topic_id
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| channel_ctx.as_ref().map(|c| c.topic_id.clone()))
        .unwrap_or_else(|| "main".to_string());

    // ① 先抢派发名额：避免「状态已改成 answered 但名额没抢到」导致确认被吞且无法重试。
    let dispatch_guard = match state.tasks.try_acquire(&session_id).await {
        Some(guard) => {
            if state.active_tasks.read().await.contains_key(&session_id) {
                guard.release().await;
                return Err(AnswerResumeError::new("任务正在执行中，请稍候再点"));
            }
            guard
        }
        None => {
            return Err(AnswerResumeError::new("任务正在执行中，请稍候再点"));
        }
    };

    // ② claim（仅飞书卡片）：防重复投递。必须在应答之前。
    if let Some(ref ctx) = channel_ctx {
        if let Err(e) = claim_card_action(
            state,
            ctx,
            interaction_id,
            &session_id,
            &chat_id,
            &topic_id,
            answer,
            operator_user_id,
            &interaction_row,
        )
        .await
        {
            dispatch_guard.release().await;
            return Err(e);
        }
    }

    // ③ 原子应答
    let answered_row = match state
        .db
        .answer_interaction(interaction_id, answer, operator_user_id)
        .await
    {
        Ok(Some(row)) => row,
        Ok(None) => {
            dispatch_guard.release().await;
            return Err(AnswerResumeError::new(
                toast_for_unanswerable(state, interaction_id, Some(&interaction_row)).await,
            ));
        }
        Err(e) => {
            dispatch_guard.release().await;
            return Err(AnswerResumeError::new(format!("应答写入失败: {e:#}")));
        }
    };

    // ④ 改终态卡（仅飞书卡片路径）
    if let Some(ref ctx) = channel_ctx {
        if let Some(card_mid) = &ctx.card_message_id {
            let question = answered_row
                .resume_ref
                .as_deref()
                .and_then(|raw| {
                    serde_json::from_str::<Value>(raw)
                        .ok()
                        .and_then(|v| v["question"].as_str().map(|s| s.to_string()))
                })
                .or_else(|| ctx.question_fallback.clone())
                .unwrap_or_else(|| "确认操作".to_string());
            let done =
                build_confirm_done_card("待确认", &question, answer, "你", Some(interaction_id));
            if let Err(e) = ctx.api.update_card(card_mid, &done).await {
                tracing::warn!("feishu: patch confirm card failed: {e:#}");
            }
        }
    }

    // ⑤ 派发：对冻结 session 跑 turn；名额 guard 传入 resume。
    let (api_for_resume, restore_card) =
        resolve_resume_api(state, &answered_row, channel_ctx.as_ref()).await;
    let card_message_id = channel_ctx
        .as_ref()
        .and_then(|c| c.card_message_id.clone())
        .or_else(|| answered_row.card_message_id.clone());
    let message_id = card_message_id.clone().unwrap_or_else(|| chat_id.clone());
    let in_thread = topic_id != "main";
    let state2 = state.clone();
    let interaction_id2 = interaction_id.to_string();
    let session_for_dispatch = session_id.clone();
    let answer2 = answer.to_string();
    let user_id2 = operator_user_id.to_string();
    let chat_id2 = chat_id;
    let topic_id2 = topic_id;

    tokio::spawn(async move {
        if let Err(e) = resume_interaction_on_session(ResumeInteraction {
            state: state2,
            api: api_for_resume,
            user_id: user_id2,
            session_id: session_for_dispatch.clone(),
            interaction_id: interaction_id2.clone(),
            text: answer2,
            message_id,
            chat_id: chat_id2,
            topic_id: topic_id2,
            in_thread,
            card_message_id,
            restore_card_on_fail: restore_card,
            dispatch_guard,
        })
        .await
        {
            tracing::warn!(
                interaction_id = %interaction_id2,
                session_id = %session_for_dispatch,
                "resume interaction failed: {e:#}"
            );
        }
    });

    Ok(())
}

/// 飞书 claim：防重复投递。claim 已存在但交互单仍 pending 时允许重试。
#[allow(clippy::too_many_arguments)]
async fn claim_card_action(
    state: &Arc<AppState>,
    ctx: &ChannelCardContext,
    interaction_id: &str,
    session_id: &str,
    chat_id: &str,
    topic_id: &str,
    answer: &str,
    operator_user_id: &str,
    interaction_row: &AgentInteraction,
) -> Result<(), AnswerResumeError> {
    let card_external_id = card_action_claim_id(
        ctx.card_message_id.as_deref(),
        ctx.event_id.as_deref(),
        chat_id,
        session_id,
    );
    let account_name = if ctx.account.name.trim().is_empty() {
        ctx.account.app_id.clone()
    } else {
        ctx.account.name.clone()
    };
    let inserted = state
        .db
        .insert_channel_message(
            "feishu",
            &ctx.account.id,
            &account_name,
            chat_id,
            topic_id,
            &card_external_id,
            ctx.card_message_id.as_deref(),
            "inbound",
            "text",
            answer,
            Some(&ctx.operator_open_id),
            Some(operator_user_id),
            Some(session_id),
            ctx.created_at,
        )
        .await
        .map_err(|e| AnswerResumeError::new(format!("claim 写入失败: {e:#}")))?;
    if !inserted {
        // 上次派发失败回滚后交互单仍是 pending，claim 仍占用 → 允许继续重试。
        let still_retryable = if interaction_row.status == "pending" {
            true
        } else {
            state
                .db
                .get_interaction(interaction_id)
                .await
                .ok()
                .flatten()
                .is_some_and(|r| r.status == "pending")
        };
        if !still_retryable {
            return Err(AnswerResumeError::new("这个操作已经提交过了"));
        }
        tracing::info!(
            interaction_id = %interaction_id,
            "feishu: claim 已存在但交互单仍 pending，允许派发重试"
        );
    }
    Ok(())
}

async fn toast_for_unanswerable(
    state: &Arc<AppState>,
    interaction_id: &str,
    cached: Option<&AgentInteraction>,
) -> String {
    let existing = match cached {
        Some(r) if r.status != "pending" => Some(r.clone()),
        _ => state
            .db
            .get_interaction(interaction_id)
            .await
            .ok()
            .flatten(),
    };
    match existing.as_ref().map(|r| r.status.as_str()) {
        Some("expired") => "待确认已超时".to_string(),
        Some(s) if s != "pending" => "这个操作已经提交过了".to_string(),
        _ => {
            if let Some(row) = existing {
                if row.expires_at.is_some_and(|t| chrono::Utc::now() > t) {
                    let _ = state
                        .db
                        .update_interaction_status(interaction_id, "expired", None, None)
                        .await;
                    "待确认已超时".to_string()
                } else {
                    "这个操作已经提交过了".to_string()
                }
            } else {
                "交互单不存在或已失效".to_string()
            }
        }
    }
}

/// admin 无 channel_ctx 时，若交互单是飞书渠道，仍解析账号以便推送进度；
/// 但不改/恢复确认卡（restore_card=false）。
async fn resolve_resume_api(
    state: &Arc<AppState>,
    row: &AgentInteraction,
    channel_ctx: Option<&ChannelCardContext>,
) -> (Option<FeishuApi>, bool) {
    if let Some(ctx) = channel_ctx {
        return (Some(ctx.api.clone()), true);
    }
    if row.channel != "feishu" {
        return (None, false);
    }
    let Some(account_id) = row.account_id.as_deref().filter(|s| !s.is_empty()) else {
        return (None, false);
    };
    match state.db.get_feishu_account(account_id).await {
        Ok(Some(account)) => {
            let api = FeishuApi::new_archived(&account, state.db.clone());
            (Some(api), false)
        }
        Ok(None) => {
            tracing::warn!(
                interaction_id = %row.id,
                account_id,
                "admin 应答：飞书账号不存在，跳过进度推送"
            );
            (None, false)
        }
        Err(e) => {
            tracing::warn!(
                interaction_id = %row.id,
                "admin 应答：读取飞书账号失败，跳过进度推送: {e:#}"
            );
            (None, false)
        }
    }
}

/// 在交互单冻结的 session 上直接 resume，不经 feishu_chats 话题映射。
/// 派发名额由调用方抢好传入；run_chat_turn 返回后 release。
async fn resume_interaction_on_session(args: ResumeInteraction) -> Result<()> {
    let ResumeInteraction {
        state,
        api,
        user_id,
        session_id,
        interaction_id,
        text,
        message_id,
        chat_id,
        topic_id,
        in_thread,
        card_message_id,
        restore_card_on_fail,
        dispatch_guard,
    } = args;

    let username = state
        .db
        .get_user_by_id(&user_id)
        .await
        .ok()
        .flatten()
        .map(|u| u.username)
        .unwrap_or_default();
    let jwt = crate::auth::sign_internal_jwt(&state.jwt_secret, &user_id, &username)
        .map_err(|e| anyhow!("签发内部 JWT 失败: {e}"))?;
    let opts = ChatTurnOpts {
        provider: None,
        model: None,
        parent_id: None,
        apply_change_id: None,
        auth_token: jwt,
        extra_prompt_segments: Vec::new(),
    };
    let content = vec![hank_provider::ContentBlock::Text { text: text.clone() }];
    let turn = run_chat_turn(&state, &session_id, content, opts).await;
    dispatch_guard.release().await;
    match turn {
        Ok(handle) => {
            if let Some(api) = api {
                pusher::spawn(
                    state.clone(),
                    api,
                    message_id,
                    chat_id,
                    topic_id,
                    session_id,
                    in_thread,
                    handle.event_rx,
                );
            }
            // broadcast 无订阅者时不阻塞发送端；无飞书推送时直接丢弃 receiver。
        }
        Err(e) => {
            // 派发失败必须回滚：否则交互单停在 answered，用户无法重试。
            tracing::warn!(
                session_id = %session_id,
                interaction_id = %interaction_id,
                "resume run_chat_turn failed, revert interaction: {e}"
            );
            state.tasks.clear_progress(&session_id).await;
            if let Err(re) = state
                .db
                .revert_interaction_to_pending(&interaction_id)
                .await
            {
                tracing::warn!(
                    interaction_id = %interaction_id,
                    "revert_interaction_to_pending failed: {re:#}"
                );
            }
            if restore_card_on_fail {
                if let Some(ref api) = api {
                    if let Err(ce) = restore_confirm_card(
                        &state,
                        api,
                        &interaction_id,
                        card_message_id.as_deref(),
                        &chat_id,
                        &topic_id,
                    )
                    .await
                    {
                        tracing::warn!(
                            interaction_id = %interaction_id,
                            "restore confirm card failed: {ce:#}"
                        );
                    }
                    let _ = api
                        .reply_text(
                            &message_id,
                            &format!("派发失败，已恢复待确认，可重新点击按钮。原因：{e}"),
                            in_thread,
                        )
                        .await;
                }
            }
        }
    }
    Ok(())
}

/// 派发失败后把终态卡片改回可点的确认卡片。
async fn restore_confirm_card(
    state: &Arc<AppState>,
    api: &FeishuApi,
    interaction_id: &str,
    card_message_id: Option<&str>,
    chat_id: &str,
    topic_id: &str,
) -> Result<()> {
    let Some(row) = state.db.get_interaction(interaction_id).await? else {
        return Ok(());
    };
    let card_mid = card_message_id
        .map(str::to_string)
        .or(row.card_message_id.clone());
    let Some(card_mid) = card_mid else {
        return Ok(());
    };
    let card = confirm_card_from_interaction(state, &row, chat_id, topic_id);
    api.update_card(&card_mid, &card).await?;
    Ok(())
}

fn confirm_card_from_interaction(
    state: &AppState,
    row: &AgentInteraction,
    chat_id: &str,
    topic_id: &str,
) -> Value {
    let resume: Value =
        serde_json::from_str(row.resume_ref.as_deref().unwrap_or("{}")).unwrap_or_default();
    let question = resume["question"]
        .as_str()
        .unwrap_or(row.title.as_str())
        .to_string();
    let choices: Vec<String> =
        serde_json::from_str(&row.options).unwrap_or_else(|_| vec!["确认".into(), "否".into()]);
    let is_quant = row.kind == "quant_confirm";
    let title = if is_quant {
        "高成本操作确认"
    } else {
        "需要你的输入"
    };
    let hint = if is_quant {
        Some(
            "点击按钮或回复文字作答；回复「确认N次」（如「确认5次」，N≤50）可批量授权本会话后续高成本操作"
                .to_string(),
        )
    } else {
        Some("点击按钮或直接回复消息作答".to_string())
    };
    let admin_url = state
        .config
        .server
        .admin_base_url
        .as_ref()
        .filter(|u| !u.trim().is_empty())
        .map(|base| format!("{}/#/interactions/{}", base.trim_end_matches('/'), row.id));
    let chat = row.chat_id.as_deref().unwrap_or(chat_id);
    let topic = row.topic_id.as_deref().unwrap_or(topic_id);
    build_confirm_card(&ConfirmCardOptions {
        title: title.to_string(),
        question,
        choices,
        interaction_id: row.id.clone(),
        session_id: row.session_id.clone(),
        chat_id: chat.to_string(),
        topic_id: topic.to_string(),
        admin_url,
        hint,
    })
}

fn card_action_claim_id(
    card_message_id: Option<&str>,
    event_id: Option<&str>,
    chat_id: &str,
    session_id: &str,
) -> String {
    let raw = if let Some(card_message_id) = card_message_id.filter(|id| !id.is_empty()) {
        format!("card-action:{card_message_id}")
    } else if let Some(event_id) = event_id.filter(|id| !id.is_empty()) {
        format!("card-action-event:{event_id}")
    } else {
        format!("card-action:{chat_id}:{session_id}")
    };
    raw.chars().take(240).collect()
}

#[cfg(test)]
mod tests {
    use super::card_action_claim_id;

    #[test]
    fn card_action_claim_is_scoped_to_the_card() {
        let first = card_action_claim_id(Some("om_card"), Some("evt_1"), "oc_1", "session_1");
        let retry = card_action_claim_id(Some("om_card"), Some("evt_2"), "oc_1", "session_1");
        assert_eq!(first, retry);
        assert_eq!(first, "card-action:om_card");
    }

    #[test]
    fn card_action_claim_falls_back_to_event_id() {
        assert_eq!(
            card_action_claim_id(None, Some("evt_1"), "oc_1", "session_1"),
            "card-action-event:evt_1"
        );
    }
}
