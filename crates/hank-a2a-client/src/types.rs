//! A2A v0.3 协议数据类型（与官方 spec 字段名对齐，camelCase）。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A2A 对话消息。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub role: String,
    #[serde(rename = "messageId")]
    pub message_id: String,
    pub parts: Vec<Part>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
}

impl Message {
    /// 构造一条 user 角色的结构化 data 消息，便于 quant skill 调用。
    pub fn data_message(skill: &str, payload: Value, metadata: Option<Map<String, Value>>) -> Self {
        let mut data = Map::new();
        data.insert("skill".to_string(), Value::String(skill.to_string()));
        data.insert("payload".to_string(), payload);

        Self {
            role: "user".to_string(),
            message_id: uuid::Uuid::new_v4().to_string(),
            parts: vec![Part::Data { data }],
            metadata,
        }
    }
}

/// Message 中的 part；A2A spec 用 `kind` 字段做 tagged union。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Part {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "data")]
    Data { data: Map<String, Value> },
    /// File part 本期不构造，但反序列化需容忍。
    #[serde(rename = "file")]
    File {
        #[serde(skip_serializing_if = "Option::is_none")]
        file_name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        bytes: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        uri: Option<String>,
    },
}

/// A2A Task。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    pub status: TaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Vec<Artifact>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<Vec<Message>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
}

/// Task 当前状态。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatus {
    pub state: TaskState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

/// Task 状态枚举；未知值落到 `Unknown(String)` 或 `Other(String)`。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum TaskState {
    Submitted,
    Working,
    Completed,
    Failed,
    Canceled,
    Rejected,
    InputRequired,
    AuthRequired,
    /// 规范未列出的状态。
    Unknown(String),
}

impl<'de> Deserialize<'de> for TaskState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "submitted" => TaskState::Submitted,
            "working" => TaskState::Working,
            "completed" => TaskState::Completed,
            "failed" => TaskState::Failed,
            "canceled" => TaskState::Canceled,
            "cancelled" => TaskState::Canceled,
            "rejected" => TaskState::Rejected,
            "inputRequired" => TaskState::InputRequired,
            "input_required" => TaskState::InputRequired,
            "authRequired" => TaskState::AuthRequired,
            "auth_required" => TaskState::AuthRequired,
            "unknown" => TaskState::Unknown("unknown".to_string()),
            other => TaskState::Unknown(other.to_string()),
        })
    }
}

impl TaskState {
    /// 是否为终态。
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TaskState::Completed
                | TaskState::Failed
                | TaskState::Canceled
                | TaskState::Rejected
                | TaskState::InputRequired
                | TaskState::AuthRequired
        )
    }
}

/// Artifact。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    #[serde(rename = "artifactId")]
    pub artifact_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parts: Vec<Part>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub append: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_chunk: Option<bool>,
}

/// `message/send` 的成功结果可能是完整 Task，也可能直接返回 Message。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum SendResult {
    Task(Task),
    Message(Message),
}

/// 流式事件：首帧通常是完整 Task，随后是 statusUpdate / artifactUpdate。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum StreamEvent {
    #[serde(rename = "task")]
    Task {
        #[serde(flatten)]
        task: Task,
    },
    #[serde(rename = "taskStatusUpdate")]
    StatusUpdate {
        #[serde(flatten)]
        status_update: TaskStatusUpdateEvent,
    },
    #[serde(rename = "taskArtifactUpdate")]
    ArtifactUpdate {
        #[serde(flatten)]
        artifact_update: TaskArtifactUpdateEvent,
    },
}

/// Task 状态更新事件。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatusUpdateEvent {
    #[serde(rename = "taskId")]
    pub task_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    pub status: TaskStatus,
    /// 部分服务端实现使用 `final`，也有用 `final` 字段名。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub final_: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
}

/// Task artifact 更新事件。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskArtifactUpdateEvent {
    #[serde(rename = "taskId")]
    pub task_id: String,
    pub artifact: Artifact,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub append: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_chunk: Option<bool>,
}

/// `tasks/list` 的返回结果。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ListTasksResult {
    pub items: Vec<Task>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_page_token: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_round_trip() {
        let msg = Message::data_message(
            "strategy.validate",
            serde_json::json!({ "spec": {} }),
            Some(
                serde_json::from_value(serde_json::json!({
                    "source": "trace_chat",
                    "trace_session_id": "sess-1"
                }))
                .unwrap(),
            ),
        );
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["role"], "user");
        assert_eq!(json["parts"][0]["kind"], "data");
        assert_eq!(json["parts"][0]["data"]["skill"], "strategy.validate");

        let back: Message = serde_json::from_value(json).unwrap();
        assert_eq!(back, msg);
    }

    #[test]
    fn test_task_state_tolerate_unknown() {
        let t: TaskStatus = serde_json::from_value(serde_json::json!({
            "state": "done",
            "message": null
        }))
        .unwrap();
        assert!(matches!(t.state, TaskState::Unknown(s) if s == "done"));
    }

    #[test]
    fn test_stream_event_status_update() {
        let raw = serde_json::json!({
            "kind": "taskStatusUpdate",
            "taskId": "task-1",
            "status": {
                "state": "working",
                "message": null
            },
            "final": true
        });
        let ev: StreamEvent = serde_json::from_value(raw).unwrap();
        match ev {
            StreamEvent::StatusUpdate { status_update } => {
                assert_eq!(status_update.task_id, "task-1");
                assert!(status_update.final_);
                assert!(matches!(status_update.status.state, TaskState::Working));
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn test_artifact_round_trip() {
        let art = Artifact {
            artifact_id: "art-1".to_string(),
            name: Some("backtest_summary".to_string()),
            description: None,
            parts: vec![Part::Data {
                data: serde_json::from_value(serde_json::json!({
                    "run_id": "r1",
                    "metrics": { "total_return": 0.12 }
                }))
                .unwrap(),
            }],
            metadata: None,
            append: None,
            last_chunk: None,
        };
        let json = serde_json::to_value(&art).unwrap();
        let back: Artifact = serde_json::from_value(json).unwrap();
        assert_eq!(back, art);
    }
}
