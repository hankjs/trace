//! JSON-RPC 2.0 信封（A2A 控制面）。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: RequestId,
    pub method: String,
    pub params: Value,
}

impl Request {
    pub fn new(id: RequestId, method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.into(),
            params,
        }
    }
}

/// JSON-RPC 响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: RequestId,
    #[serde(flatten)]
    pub payload: ResponsePayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponsePayload {
    Result { result: Value },
    Error { error: RpcError },
}

/// JSON-RPC 错误体。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// 请求 ID：字符串或整数（A2A 通常用字符串）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum RequestId {
    String(String),
    Number(i64),
    Null,
}

impl RequestId {
    pub fn string(value: impl Into<String>) -> Self {
        RequestId::String(value.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_response_parse() {
        let raw = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "req-1",
            "error": {
                "code": -32001,
                "message": "Task not found",
                "data": { "task_id": "t1" }
            }
        });
        let resp: Response = serde_json::from_value(raw).unwrap();
        match resp.payload {
            ResponsePayload::Error { error } => {
                assert_eq!(error.code, -32001);
                assert_eq!(error.message, "Task not found");
                assert_eq!(error.data.unwrap()["task_id"], "t1");
            }
            _ => panic!("expected error"),
        }
    }

    #[test]
    fn test_request_serialize() {
        let req = Request::new(
            RequestId::string("req-1"),
            "tasks/get",
            serde_json::json!({ "id": "task-1" }),
        );
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], "req-1");
        assert_eq!(v["method"], "tasks/get");
    }
}
