//! 卡片按钮回调：card.action.trigger → 按 interaction_id 原子应答 → 派发 resume。
//!
//! 交互单落表后，确认不再寄生在 session 上：按钮 callback 携带 interaction_id，
//! 用 answer_interaction 原子抢答；派发时强制使用交互单上冻结的 session_id，
//! 即使飞书话题映射已重建到新 session，也能回到正确的待确认会话。
//!
//! 顺序必须是 **抢名额 → claim → 应答 → 改卡片 → 派发**：
//! 若先应答再抢名额，会留下「answered 但未派发」的不可恢复僵尸。

use crate::chat::{run_chat_turn, ChatTurnOpts};
use crate::feishu::api::FeishuApi;
use crate::feishu::card::{build_confirm_card, build_confirm_done_card, ConfirmCardOptions};
use crate::feishu::pusher;
use crate::feishu::router::{self, IncomingMessage};
use crate::task_state::DispatchGuard;
use crate::AppState;
use anyhow::{anyhow, Result};
use hank_db::{AgentInteraction, FeishuAccount};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct CardActionEvent {
    operator: Option<Operator>,
    action: Option<ActionValue>,
    context: Option<ActionContext>,
}

#[derive(Debug, Deserialize, Clone)]
struct Operator {
    open_id: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct ActionValue {
    value: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ActionContext {
    open_message_id: Option<String>,
}

/// resume 入参打包，避免 too_many_arguments。
struct ResumeInteraction {
    state: Arc<AppState>,
    api: FeishuApi,
    user_id: String,
    session_id: String,
    interaction_id: String,
    text: String,
    message_id: String,
    chat_id: String,
    topic_id: String,
    in_thread: bool,
    /// 点击事件上的卡片 message id，派发失败时用于恢复可点卡片
    card_message_id: Option<String>,
    dispatch_guard: DispatchGuard,
}

/// 处理一次卡片回调，返回给飞书的响应 data（toast）。
pub async fn handle_card_action(
    state: Arc<AppState>,
    account: FeishuAccount,
    payload: &[u8],
) -> Result<Value> {
    // 外层 envelope（schema 2.0）：{"schema","header","event":{...}}；容错扁平结构
    let root: Value = serde_json::from_slice(payload)?;
    let event_id = root["header"]["event_id"].as_str().map(str::to_string);
    let created_at = router::parse_feishu_timestamp(root["header"]["create_time"].as_str())
        .unwrap_or_else(chrono::Utc::now);
    let body = if root.get("event").is_some() {
        root["event"].clone()
    } else {
        root.clone()
    };
    let ev: CardActionEvent = serde_json::from_value(body)?;

    let operator_open_id = ev
        .operator
        .clone()
        .and_then(|o| o.open_id)
        .unwrap_or_default();
    let value = ev
        .action
        .clone()
        .and_then(|a| a.value)
        .ok_or_else(|| anyhow!("卡片回调缺少 action.value"))?;

    if value["action"].as_str() == Some("deploy_approval") {
        return handle_deploy_approval(
            state,
            account,
            operator_open_id,
            value,
            card_message_id_from_event(&ev),
        )
        .await;
    }

    // 只认我们发出的 answer 按钮
    if value["action"].as_str() != Some("answer") {
        tracing::debug!(value = %value, "feishu: 忽略非 answer 卡片回调");
        return Ok(json!({}));
    }
    let choice = value["choice"]
        .as_str()
        .ok_or_else(|| anyhow!("卡片回调缺少 choice"))?
        .to_string();
    let interaction_id = value["interaction_id"].as_str().unwrap_or("").to_string();
    let session_id_from_card = value["session_id"].as_str().unwrap_or("").to_string();
    let chat_id = value["chat_id"].as_str().unwrap_or("").to_string();
    let topic_id = value["topic_id"].as_str().unwrap_or("").to_string();
    let card_message_id = ev.context.and_then(|c| c.open_message_id);

    let api = FeishuApi::new_archived(&account, state.db.clone());

    // 操作者必须是已绑定用户
    let binding = state
        .db
        .get_feishu_binding(&account.id, &operator_open_id)
        .await
        .unwrap_or(None);
    let Some(binding) = binding else {
        tracing::warn!(open_id = %operator_open_id, "feishu: 未绑定用户点击了卡片按钮");
        return Ok(json!({
            "toast": { "type": "warning", "content": "你还没有绑定，请先发送 bind 绑定码" }
        }));
    };
    let user_id = binding.user_id.clone();

    tracing::info!(
        event_id = event_id.as_deref().unwrap_or(""),
        card_message_id = card_message_id.as_deref().unwrap_or(""),
        operator = %operator_open_id,
        choice = %choice,
        interaction_id = %interaction_id,
        session_id = %session_id_from_card,
        "feishu: 卡片按钮点击"
    );

    // 先取交互单以拿到冻结的 session_id（派发与 claim 都要用）
    let interaction_row = if !interaction_id.is_empty() {
        state.db.get_interaction(&interaction_id).await?
    } else {
        None
    };
    // 派发必须落到交互单冻结的 session_id，而不是 feishu_chats 当前映射
    // （话题 Recreate 后映射已指向新 session，会把确认投给孤儿会话）。
    let session_id = interaction_row
        .as_ref()
        .map(|r| r.session_id.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or(session_id_from_card.clone());

    // ① 先抢派发名额：避免「状态已改成 answered 但名额没抢到」导致确认被吞且无法重试。
    // 名额抢不到说明该会话确实在忙，此时不应答、不 claim、不改卡片，用户可稍后再点。
    let dispatch_guard = if !interaction_id.is_empty() && !session_id.is_empty() {
        match state.tasks.try_acquire(&session_id).await {
            Some(guard) => {
                if state.active_tasks.read().await.contains_key(&session_id) {
                    guard.release().await;
                    return Ok(json!({
                        "toast": { "type": "warning", "content": "任务正在执行中，请稍候再点" }
                    }));
                }
                Some(guard)
            }
            None => {
                return Ok(json!({
                    "toast": { "type": "warning", "content": "任务正在执行中，请稍候再点" }
                }));
            }
        }
    } else {
        None
    };

    let in_thread = topic_id != "main";
    let msg = IncomingMessage {
        message_id: card_message_id.clone().unwrap_or_else(|| chat_id.clone()),
        chat_id: chat_id.clone(),
        message_type: "text".to_string(),
        text: choice.clone(),
        root_id: String::new(),
        thread_id: if in_thread {
            topic_id.clone()
        } else {
            String::new()
        },
        sender_open_id: operator_open_id,
    };
    let card_external_id = card_action_claim_id(
        card_message_id.as_deref(),
        event_id.as_deref(),
        &chat_id,
        &session_id,
    );
    let account_name = if account.name.trim().is_empty() {
        account.app_id.clone()
    } else {
        account.name.clone()
    };
    // ② claim：防飞书重复投递。必须在应答之前；抢名额失败已 early return，不会占 claim。
    let inserted = state
        .db
        .insert_channel_message(
            "feishu",
            &account.id,
            &account_name,
            &msg.chat_id,
            &msg.topic_id(),
            &card_external_id,
            card_message_id.as_deref(),
            "inbound",
            "text",
            &choice,
            Some(&msg.sender_open_id),
            Some(&user_id),
            (!session_id.is_empty()).then_some(session_id.as_str()),
            created_at,
        )
        .await?;
    if !inserted {
        // 上次派发失败回滚后交互单仍是 pending，claim 仍占用 → 允许继续重试。
        // 真正已处理（answered/done/…）的重复投递仍拒绝。
        let still_retryable = match interaction_row.as_ref().map(|r| r.status.as_str()) {
            Some("pending") => true,
            _ => {
                // 再读一次，覆盖「启动收尾刚把 answered 僵尸退回 pending」等竞态
                state
                    .db
                    .get_interaction(&interaction_id)
                    .await?
                    .is_some_and(|r| r.status == "pending")
            }
        };
        if !still_retryable {
            // 释放已抢到的名额，避免会话被永久占住
            if let Some(guard) = dispatch_guard {
                guard.release().await;
            }
            return Ok(json!({
                "toast": { "type": "warning", "content": "这个操作已经提交过了" }
            }));
        }
        tracing::info!(
            interaction_id = %interaction_id,
            "feishu: claim 已存在但交互单仍 pending，允许派发重试"
        );
    }

    // ③ 按 interaction_id 原子应答
    let answered_row = if !interaction_id.is_empty() {
        match state
            .db
            .answer_interaction(&interaction_id, &choice, &user_id)
            .await?
        {
            Some(row) => Some(row),
            None => {
                if let Some(guard) = dispatch_guard {
                    guard.release().await;
                }
                let existing =
                    interaction_row.or(state.db.get_interaction(&interaction_id).await?);
                let toast = match existing.as_ref().map(|r| r.status.as_str()) {
                    Some("expired") => "待确认已超时",
                    Some(s) if s != "pending" => "这个操作已经提交过了",
                    _ => {
                        if let Some(row) = existing {
                            if row.expires_at.is_some_and(|t| chrono::Utc::now() > t) {
                                let _ = state
                                    .db
                                    .update_interaction_status(
                                        &interaction_id,
                                        "expired",
                                        None,
                                        None,
                                    )
                                    .await;
                                "待确认已超时"
                            } else {
                                "这个操作已经提交过了"
                            }
                        } else {
                            "交互单不存在或已失效"
                        }
                    }
                };
                return Ok(json!({
                    "toast": { "type": "warning", "content": toast }
                }));
            }
        }
    } else {
        None
    };

    // 终态卡片：question 从交互单 resume_ref 读回，不再依赖 callback payload
    let question = answered_row
        .as_ref()
        .and_then(|r| {
            r.resume_ref.as_deref().and_then(|raw| {
                serde_json::from_str::<Value>(raw)
                    .ok()
                    .and_then(|v| v["question"].as_str().map(|s| s.to_string()))
            })
        })
        .or_else(|| value["question"].as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "确认操作".to_string());

    // ④ claim + 应答成功后再改卡片
    if let Some(card_mid) = &card_message_id {
        let done = build_confirm_done_card(
            "待确认",
            &question,
            &choice,
            "你",
            (!interaction_id.is_empty()).then_some(interaction_id.as_str()),
        );
        if let Err(e) = api.update_card(card_mid, &done).await {
            tracing::warn!("feishu: patch confirm card failed: {e:#}");
        }
    }

    // ⑤ 派发：有 interaction_id 时直接对冻结 session 跑 turn；名额 guard 传入 resume。
    let state2 = state.clone();
    let api2 = api.clone();
    let choice_for_dispatch = choice.clone();
    let session_for_dispatch = session_id.clone();
    let account2 = account.clone();
    let interaction_id2 = interaction_id.clone();
    let chat_id2 = chat_id.clone();
    let topic_id2 = topic_id.clone();
    let message_id2 = msg.message_id.clone();
    let card_mid2 = card_message_id.clone();
    let in_thread2 = in_thread;
    tokio::spawn(async move {
        if let Some(guard) = dispatch_guard {
            if let Err(e) = resume_interaction_on_session(ResumeInteraction {
                state: state2,
                api: api2,
                user_id,
                session_id: session_for_dispatch.clone(),
                interaction_id: interaction_id2.clone(),
                text: choice_for_dispatch,
                message_id: message_id2,
                chat_id: chat_id2,
                topic_id: topic_id2,
                in_thread: in_thread2,
                card_message_id: card_mid2,
                dispatch_guard: guard,
            })
            .await
            {
                tracing::warn!(
                    interaction_id = %interaction_id2,
                    session_id = %session_for_dispatch,
                    "feishu: resume interaction failed: {e:#}"
                );
            }
        } else if let Err(e) = router::dispatch_task(
            &state2,
            &api2,
            &account2,
            &msg,
            &user_id,
            &choice_for_dispatch,
        )
        .await
        {
            tracing::warn!("feishu: dispatch from card action failed: {e:#}");
        }
    });

    Ok(json!({
        "toast": { "type": "success", "content": format!("已提交：{choice}") }
    }))
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
            pusher::spawn(
                state.clone(),
                api.clone(),
                message_id,
                chat_id,
                topic_id,
                session_id,
                in_thread,
                handle.event_rx,
            );
        }
        Err(e) => {
            // 派发失败必须回滚：否则交互单停在 answered，用户无法重试。
            // 退回 pending 而不是标 failed——用户的确认意图是真实的。
            tracing::warn!(
                session_id = %session_id,
                interaction_id = %interaction_id,
                "feishu: resume run_chat_turn failed, revert interaction: {e}"
            );
            state.tasks.clear_progress(&session_id).await;
            if let Err(re) = state
                .db
                .revert_interaction_to_pending(&interaction_id)
                .await
            {
                tracing::warn!(
                    interaction_id = %interaction_id,
                    "feishu: revert_interaction_to_pending failed: {re:#}"
                );
            }
            // 卡片改回可点；失败只 warn，不因改卡失败丢掉回滚
            if let Err(ce) = restore_confirm_card(
                &state,
                &api,
                &interaction_id,
                card_message_id.as_deref(),
                &chat_id,
                &topic_id,
            )
            .await
            {
                tracing::warn!(
                    interaction_id = %interaction_id,
                    "feishu: restore confirm card failed: {ce:#}"
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

async fn handle_deploy_approval(
    state: Arc<AppState>,
    account: FeishuAccount,
    operator_open_id: String,
    value: Value,
    card_message_id: Option<String>,
) -> Result<Value> {
    let deployment_id = value["deployment_id"].as_str().unwrap_or_default();
    let decision = value["decision"].as_str().unwrap_or_default();
    if deployment_id.is_empty() || !matches!(decision, "approve" | "cancel") {
        return Ok(json!({
            "toast": { "type": "warning", "content": "无效的部署审批" }
        }));
    }
    let binding = state
        .db
        .get_feishu_binding(&account.id, &operator_open_id)
        .await
        .unwrap_or(None);
    let Some(binding) = binding else {
        return Ok(json!({
            "toast": { "type": "warning", "content": "你还没有绑定 Trace 用户" }
        }));
    };
    if let Err(e) = crate::deployment::ensure_server_agent_admin(&state, &binding.user_id).await {
        return Ok(json!({
            "toast": { "type": "warning", "content": e.to_string() }
        }));
    }
    let Some(mut deployment) = state.db.get_deployment(deployment_id).await? else {
        return Ok(json!({
            "toast": { "type": "warning", "content": "部署任务不存在或已清理" }
        }));
    };
    if deployment.account_id != account.id {
        return Ok(json!({
            "toast": { "type": "warning", "content": "审批卡片不属于当前飞书应用" }
        }));
    }
    if deployment.user_id != binding.user_id {
        return Ok(json!({
            "toast": { "type": "warning", "content": "只有发起人可以审批此部署" }
        }));
    }
    if let (Some(expected), Some(actual)) = (
        deployment.card_message_id.as_deref(),
        card_message_id.as_deref(),
    ) {
        if expected != actual {
            return Ok(json!({
                "toast": { "type": "warning", "content": "审批卡片与部署任务不匹配" }
            }));
        }
    }
    if let Some(card_id) = card_message_id.as_deref() {
        let _ = state.db.set_deployment_card(deployment_id, card_id).await;
        deployment.card_message_id = Some(card_id.to_string());
    }
    let api = FeishuApi::new_archived(&account, state.db.clone());
    if decision == "cancel" {
        if !state
            .db
            .cancel_deployment(deployment_id, &binding.user_id)
            .await?
        {
            return Ok(json!({
                "toast": { "type": "warning", "content": "部署已处理、已过期或已被其他操作占用" }
            }));
        }
        if let Some(card_id) = card_message_id.as_deref() {
            let card =
                build_confirm_done_card("Trace 发布", &deployment.summary, "已取消", "你", None);
            let _ = api.update_card(card_id, &card).await;
        }
        return Ok(json!({
            "toast": { "type": "success", "content": "部署已取消" }
        }));
    }

    let Some(approved) = state
        .db
        .approve_deployment(deployment_id, &binding.user_id)
        .await?
    else {
        return Ok(json!({
            "toast": { "type": "warning", "content": "部署已处理、已过期或已被其他操作占用" }
        }));
    };
    if let Some(card_id) = card_message_id.as_deref() {
        let card = build_confirm_done_card(
            "Trace 发布",
            &approved.summary,
            "已批准，正在执行",
            "你",
            None,
        );
        let _ = api.update_card(card_id, &card).await;
    }
    tokio::spawn(crate::deployment::start_deployment(state, approved));
    Ok(json!({
        "toast": { "type": "success", "content": "部署已批准，正在执行" }
    }))
}

fn card_message_id_from_event(ev: &CardActionEvent) -> Option<String> {
    ev.context
        .as_ref()
        .and_then(|context| context.open_message_id.clone())
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
