//! A2A v0.3 最小客户端。

use futures::Stream;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::jsonrpc::{Request, RequestId, Response, ResponsePayload};
use crate::sse::decode_sse_stream;
use crate::types::{ListTasksResult, Message, SendResult, StreamEvent, Task};

const A2A_VERSION: &str = "0.3";

/// A2A 客户端；内部复用 reqwest::Client。
#[derive(Debug, Clone)]
pub struct A2aClient {
    rpc_url: String,
    bearer_token: String,
    client: reqwest::Client,
}

impl A2aClient {
    /// 创建客户端。既接受服务根地址，也接受完整 `/a2a` RPC 地址。
    pub fn new(base_url: impl Into<String>, bearer_token: impl Into<String>) -> Self {
        let base_url = base_url.into();
        let trimmed = base_url.trim_end_matches('/');
        let rpc_url = if trimmed.ends_with("/a2a") {
            trimmed.to_string()
        } else {
            format!("{trimmed}/a2a")
        };
        Self {
            rpc_url,
            bearer_token: bearer_token.into(),
            client: reqwest::Client::new(),
        }
    }

    /// 构造 data part 消息（见 `Message::data_message`）。
    pub fn data_message(
        skill: &str,
        payload: Value,
        metadata: Option<Map<String, Value>>,
    ) -> Message {
        Message::data_message(skill, payload, metadata)
    }

    /// 发送非流式消息，返回完整 Task 或直接 Message。
    pub async fn send_message(&self, msg: Message) -> Result<SendResult> {
        let params = serde_json::to_value(&SendMessageParams { message: msg })?;
        let value = self.call("message/send", params).await?;
        parse_send_result(value)
    }

    /// 发送流式消息，返回 SSE 事件流。
    pub async fn send_streaming_message(
        &self,
        msg: Message,
    ) -> Result<impl Stream<Item = Result<StreamEvent>>> {
        let params = serde_json::to_value(&SendMessageParams { message: msg })?;
        self.post_sse("message/stream", params).await
    }

    /// 查询 Task。
    pub async fn get_task(&self, task_id: &str) -> Result<Task> {
        let params = serde_json::json!({ "id": task_id });
        let value = self.call("tasks/get", params).await?;
        Ok(serde_json::from_value(value)?)
    }

    /// 取消 Task。
    pub async fn cancel_task(&self, task_id: &str) -> Result<Task> {
        let params = serde_json::json!({ "id": task_id });
        let value = self.call("tasks/cancel", params).await?;
        Ok(serde_json::from_value(value)?)
    }

    /// 列出 Task（仅长任务）。
    pub async fn list_tasks(
        &self,
        page_token: Option<&str>,
        page_size: Option<i32>,
    ) -> Result<ListTasksResult> {
        let mut params = serde_json::Map::new();
        if let Some(token) = page_token {
            params.insert("pageToken".to_string(), Value::String(token.to_string()));
        }
        if let Some(size) = page_size {
            params.insert("pageSize".to_string(), Value::Number(size.into()));
        }
        let value = self.call("tasks/list", Value::Object(params)).await?;
        Ok(serde_json::from_value(value)?)
    }

    /// 断线后续订长任务。
    pub async fn resubscribe(
        &self,
        task_id: &str,
    ) -> Result<impl Stream<Item = Result<StreamEvent>>> {
        let params = serde_json::json!({ "id": task_id });
        self.post_sse("tasks/resubscribe", params).await
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let id = RequestId::string(uuid::Uuid::new_v4().to_string());
        let req = Request::new(id, method, params);
        let resp: Response = self
            .client
            .post(&self.rpc_url)
            .headers(self.headers())
            .json(&req)
            .send()
            .await?
            .json()
            .await?;

        match resp.payload {
            ResponsePayload::Result { result } => Ok(result),
            ResponsePayload::Error { error } => Err(Error::Rpc {
                code: error.code,
                message: error.message,
                data: error.data,
            }),
        }
    }

    async fn post_sse(
        &self,
        method: &str,
        params: Value,
    ) -> Result<impl Stream<Item = Result<StreamEvent>>> {
        let id = RequestId::string(uuid::Uuid::new_v4().to_string());
        let req = Request::new(id, method, params);
        let resp = self
            .client
            .post(&self.rpc_url)
            .headers(self.headers())
            .json(&req)
            .send()
            .await?;

        resp.error_for_status_ref()?;
        Ok(decode_sse_stream(resp.bytes_stream()))
    }

    fn headers(&self) -> reqwest::header::HeaderMap {
        use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
        headers.insert(
            reqwest::header::HeaderName::from_static("a2a-version"),
            HeaderValue::from_static(A2A_VERSION),
        );
        let auth = format!("Bearer {}", self.bearer_token);
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&auth).expect("valid bearer token header"),
        );
        headers
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SendMessageParams {
    message: Message,
}

fn parse_send_result(value: Value) -> Result<SendResult> {
    if value.get("id").is_some() && value.get("status").is_some() {
        return Ok(SendResult::Task(serde_json::from_value(value)?));
    }
    if value.get("role").is_some() && value.get("messageId").is_some() {
        return Ok(SendResult::Message(serde_json::from_value(value)?));
    }

    // 兜底：尝试按 untagged 枚举反序列化。
    serde_json::from_value(value).map_err(|e| Error::UnexpectedResult(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Part;

    #[test]
    fn test_parse_send_result_task() {
        let raw = serde_json::json!({
            "id": "task-1",
            "status": { "state": "working" }
        });
        let res = parse_send_result(raw).unwrap();
        assert!(matches!(res, SendResult::Task(_)));
    }

    #[test]
    fn test_parse_send_result_message() {
        let raw = serde_json::json!({
            "role": "agent",
            "messageId": "m1",
            "parts": [{ "kind": "text", "text": "done" }]
        });
        let res = parse_send_result(raw).unwrap();
        assert!(matches!(res, SendResult::Message(_)));
    }

    #[test]
    fn test_data_message_builder() {
        let msg = A2aClient::data_message(
            "backtest.run",
            serde_json::json!({ "strategy_id": 1 }),
            None,
        );
        assert_eq!(msg.role, "user");
        assert_eq!(msg.parts.len(), 1);
        match &msg.parts[0] {
            Part::Data { data } => assert_eq!(data["skill"], "backtest.run"),
            _ => panic!("expected data part"),
        }
    }

    #[test]
    fn test_rpc_url_accepts_service_root_or_full_endpoint() {
        for (input, expected) in [
            ("http://127.0.0.1:8100", "http://127.0.0.1:8100/a2a"),
            ("http://127.0.0.1:8100/", "http://127.0.0.1:8100/a2a"),
            ("http://127.0.0.1:8100/a2a", "http://127.0.0.1:8100/a2a"),
            ("http://127.0.0.1:8100/a2a/", "http://127.0.0.1:8100/a2a"),
        ] {
            let client = A2aClient::new(input, "token");
            assert_eq!(client.rpc_url, expected);
        }
    }
}
