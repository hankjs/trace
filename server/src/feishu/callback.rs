//! 卡片按钮回调：card.action.trigger → 包装成文本回复走现有确认闸门。
//!
//! 文档里「审批卡片」的 v1 形态：用户点按钮等价于在微信里回复"确认"，
//! chat.rs 的 resolve_pending_ask_user / handle_quant_confirmation 原样接住，
//! code-agent 零改动。

use crate::feishu::api::FeishuApi;
use crate::feishu::card::build_confirm_done_card;
use crate::feishu::router::{self, IncomingMessage};
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

#[derive(Debug, Deserialize)]
struct Operator {
    open_id: Option<String>,
}

#[derive(Debug, Deserialize)]
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
        .and_then(|o| o.open_id)
        .unwrap_or_default();
    let value = ev
        .action
        .and_then(|a| a.value)
        .ok_or_else(|| anyhow!("卡片回调缺少 action.value"))?;

    // 只认我们发出的 answer 按钮
    if value["action"].as_str() != Some("answer") {
        tracing::debug!(value = %value, "feishu: 忽略非 answer 卡片回调");
        return Ok(json!({}));
    }
    let choice = value["choice"]
        .as_str()
        .ok_or_else(|| anyhow!("卡片回调缺少 choice"))?
        .to_string();
    let session_id = value["session_id"].as_str().unwrap_or("").to_string();
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
        session_id = %session_id,
        "feishu: 卡片按钮点击"
    );

    // 构造一条"虚拟用户消息"走正常派发：确认/否/选项文本 → 现有确认解析层接住
    let in_thread = topic_id != "main";
    let msg = IncomingMessage {
        message_id: card_message_id.clone().unwrap_or_else(|| chat_id.clone()),
        chat_id: chat_id.clone(),
        message_type: "text".to_string(),
        text: choice.clone(),
        root_id: String::new(),
        thread_id: if in_thread { topic_id } else { String::new() },
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
        return Ok(json!({
            "toast": { "type": "warning", "content": "这个操作已经提交过了" }
        }));
    }

    // claim 成功后再改卡片，重复或冲突点击不能覆盖首次选择的终态。
    if let Some(card_mid) = &card_message_id {
        let question = value["question"].as_str().unwrap_or("确认操作");
        let done = build_confirm_done_card("待确认", question, &choice, "你");
        if let Err(e) = api.update_card(card_mid, &done).await {
            tracing::warn!("feishu: patch confirm card failed: {e:#}");
        }
    }

    let state2 = state.clone();
    let api2 = api.clone();
    let choice_for_dispatch = choice.clone();
    tokio::spawn(async move {
        if let Err(e) =
            router::dispatch_task(&state2, &api2, &account, &msg, &user_id, &choice_for_dispatch).await
        {
            tracing::warn!("feishu: dispatch from card action failed: {e:#}");
        }
    });

    Ok(json!({
        "toast": { "type": "success", "content": format!("已提交：{choice}") }
    }))
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
