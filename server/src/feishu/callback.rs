//! 卡片按钮回调：card.action.trigger → 按 interaction_id 原子应答 → 派发 resume。
//!
//! 交互单落表后，确认不再寄生在 session 上：按钮 callback 携带 interaction_id，
//! 用 answer_interaction 原子抢答；派发时强制使用交互单上冻结的 session_id，
//! 即使飞书话题映射已重建到新 session，也能回到正确的待确认会话。
//!
//! 顺序必须是 **抢名额 → claim → 应答 → 改卡片 → 派发**（见 `interaction_flow`）：
//! 若先应答再抢名额，会留下「answered 但未派发」的不可恢复僵尸。
//! 飞书回调与 admin 手动应答共用 `answer_and_resume`，避免两处顺序漂移。

use crate::feishu::api::FeishuApi;
use crate::feishu::card::build_confirm_done_card;
use crate::feishu::router::{self, IncomingMessage};
use crate::interaction_flow::{self, ChannelCardContext};
use crate::AppState;
use anyhow::{anyhow, Result};
use hank_db::FeishuAccount;
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

    // 有 interaction_id：走共用 answer_and_resume（抢名额 → claim → 应答 → 改卡 → 派发）
    if !interaction_id.is_empty() {
        let question_fallback = value["question"].as_str().map(|s| s.to_string());
        match interaction_flow::answer_and_resume(
            &state,
            &interaction_id,
            &choice,
            &user_id,
            Some(ChannelCardContext {
                api,
                account,
                card_message_id,
                event_id,
                operator_open_id,
                created_at,
                chat_id,
                topic_id,
                question_fallback,
            }),
        )
        .await
        {
            Ok(()) => Ok(json!({
                "toast": { "type": "success", "content": format!("已提交：{choice}") }
            })),
            Err(e) => Ok(json!({
                "toast": { "type": "warning", "content": e.message }
            })),
        }
    } else {
        // 无 interaction_id 的旧卡片：退回话题映射派发（无原子应答保护）
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
        let state2 = state.clone();
        let api2 = api.clone();
        let account2 = account.clone();
        let choice_for_dispatch = choice.clone();
        tokio::spawn(async move {
            if let Err(e) = router::dispatch_task(
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
