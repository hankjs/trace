//! Hank A2A v0.3 最小客户端。
//!
//! 提供 JSON-RPC over HTTP + SSE 的 A2A 协议子集，供 server 侧 quant 工具调用 quant A2A Server。
//! 本 crate 不耦合 quant 业务：skill / payload 由调用方以 `serde_json::Value` 传入。

pub mod client;
pub mod error;
pub mod jsonrpc;
pub mod sse;
pub mod types;

pub use client::A2aClient;
pub use error::{Error, Result};
pub use types::{
    Artifact, ListTasksResult, Message, Part, SendResult, StreamEvent, Task,
    TaskArtifactUpdateEvent, TaskState, TaskStatus, TaskStatusUpdateEvent,
};

// 为 A2A SDK 风格的 stream event 提供别名。
pub use types::StreamEvent as A2aStreamEvent;
