//! 渠道聊天记录管理接口。
//!
//! 目前只开放飞书，接口和数据库模型按渠道维度设计，后续微信接入时无需重做管理端。

use crate::admin::PaginatedResponse;
use crate::response::{self as R};
use crate::AppState;
use axum::{
    extract::{Query, State},
    response::IntoResponse,
};
use serde::Deserialize;
use std::sync::Arc;

const DEFAULT_CHANNEL: &str = "feishu";

#[derive(Debug, Deserialize)]
pub struct ConversationQuery {
    pub channel: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
    pub search: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MessagesQuery {
    pub channel: Option<String>,
    pub account_id: String,
    pub conversation_id: String,
    pub topic_id: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

fn supports_channel(channel: Option<&str>) -> bool {
    channel
        .unwrap_or(DEFAULT_CHANNEL)
        .trim()
        .eq_ignore_ascii_case(DEFAULT_CHANNEL)
}

pub async fn list_conversations(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ConversationQuery>,
) -> impl IntoResponse {
    if !supports_channel(query.channel.as_deref()) {
        return R::bad_request("暂时只支持飞书聊天记录");
    }
    let channel = DEFAULT_CHANNEL;
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(30).clamp(1, 100);
    let search = query.search.as_deref().unwrap_or("").trim();
    let total = match state.db.count_channel_conversations(channel, search).await {
        Ok(total) => total,
        Err(e) => return R::internal_error(e),
    };
    let data = match state
        .db
        .list_channel_conversations(channel, search, page, per_page)
        .await
    {
        Ok(data) => data,
        Err(e) => return R::internal_error(e),
    };
    R::ok(PaginatedResponse {
        data,
        total,
        page,
        per_page,
    })
}

pub async fn list_messages(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MessagesQuery>,
) -> impl IntoResponse {
    if !supports_channel(query.channel.as_deref()) {
        return R::bad_request("暂时只支持飞书聊天记录");
    }
    let channel = DEFAULT_CHANNEL;
    let account_id = query.account_id.trim();
    let conversation_id = query.conversation_id.trim();
    let topic_id = query.topic_id.as_deref().unwrap_or("main").trim();
    if account_id.is_empty() || conversation_id.is_empty() || topic_id.is_empty() {
        return R::bad_request("account_id、conversation_id、topic_id 不能为空");
    }
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(100).clamp(1, 200);
    let (data, total) = match state
        .db
        .list_channel_messages(
            channel,
            account_id,
            conversation_id,
            topic_id,
            page,
            per_page,
        )
        .await
    {
        Ok(result) => result,
        Err(e) => return R::internal_error(e),
    };
    R::ok(PaginatedResponse {
        data,
        total,
        page,
        per_page,
    })
}
