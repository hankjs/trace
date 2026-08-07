//! 进程内 hub：pending 队列 + inflight oneshot + 在线观测。

use crate::AppState;
use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, Notify};

/// 在线判定窗口：该时长内有 poll 即在线
pub const ONLINE_WINDOW: Duration = Duration::from_secs(60);

/// 纯观察类工具：admin 页高频轮询不计入「最后运行」
const OBSERVE_ONLY_TOOLS: &[&str] = &["terminal_list", "terminal_read"];

fn counts_as_dispatch(tool: &str) -> bool {
    !OBSERVE_ONLY_TOOLS.contains(&tool)
}

/// 一条待 client 执行的工具调用
#[derive(Debug, Clone, Serialize)]
pub struct ToolCallRequest {
    pub request_id: String,
    /// 目标 client（仅 server 内部路由，不下发给 client）
    #[serde(skip)]
    pub client_id: String,
    pub tool: String,
    pub input: Value,
}

/// client 回传的工具执行结果
#[derive(Debug, Clone)]
pub struct ToolCallResult {
    pub content: String,
    pub is_error: bool,
}

/// 单个用户的 client 通道（挂 AppState.client_hubs）
#[derive(Default)]
pub struct UserHub {
    pub pending: VecDeque<ToolCallRequest>,
    pub notify: Arc<Notify>,
    pub inflight: HashMap<String, oneshot::Sender<ToolCallResult>>,
    pub last_polls: HashMap<String, Instant>,
}

impl UserHub {
    pub fn notify_handle(&mut self) -> Arc<Notify> {
        self.notify.clone()
    }

    /// 取走该 client 的全部待执行请求；顺带刷新在线时间
    pub fn drain_for(&mut self, client_id: &str) -> Vec<ToolCallRequest> {
        self.last_polls
            .insert(client_id.to_string(), Instant::now());
        let (mine, rest): (VecDeque<_>, VecDeque<_>) = self
            .pending
            .drain(..)
            .partition(|r| r.client_id == client_id);
        self.pending = rest;
        mine.into_iter().collect()
    }

    pub fn is_online(&self, client_id: &str) -> bool {
        self.last_polls
            .get(client_id)
            .is_some_and(|t| t.elapsed() < ONLINE_WINDOW)
    }
}

/// 入队一条工具调用并等待 client 回传结果。超时/断线返回 Err。
pub async fn dispatch_tool_call(
    state: &AppState,
    user_id: &str,
    client_id: &str,
    tool: &str,
    input: Value,
    timeout: Duration,
) -> Result<ToolCallResult> {
    let request_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel();
    {
        let mut hubs = state.client_hubs.write().await;
        let hub = hubs.entry(user_id.to_string()).or_default();
        hub.pending.push_back(ToolCallRequest {
            request_id: request_id.clone(),
            client_id: client_id.to_string(),
            tool: tool.to_string(),
            input,
        });
        hub.inflight.insert(request_id.clone(), tx);
        hub.notify.notify_one();
    }

    if counts_as_dispatch(tool) {
        let _ = state.db.touch_client_agent_active(client_id).await;
    }

    let result = tokio::time::timeout(timeout, rx).await;

    {
        let mut hubs = state.client_hubs.write().await;
        if let Some(hub) = hubs.get_mut(user_id) {
            hub.inflight.remove(&request_id);
        }
    }

    match result {
        Ok(Ok(r)) => Ok(r),
        Ok(Err(_)) => Err(anyhow!("client 连接中断")),
        Err(_) => Err(anyhow!(
            "等待 client 执行结果超时（{}s）",
            timeout.as_secs()
        )),
    }
}

/// 指定 client 是否在线（60s 内有 poll）
pub async fn is_client_online(state: &AppState, user_id: &str, client_id: &str) -> bool {
    let hubs = state.client_hubs.read().await;
    hubs.get(user_id)
        .is_some_and(|hub| hub.is_online(client_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_tools_do_not_count_as_dispatch() {
        assert!(!counts_as_dispatch("terminal_list"));
        assert!(!counts_as_dispatch("terminal_read"));
        assert!(counts_as_dispatch("terminal_write"));
        assert!(counts_as_dispatch("rtc_signal"));
    }

    #[test]
    fn drain_for_only_returns_matching_client() {
        let mut hub = UserHub::default();
        hub.pending.push_back(ToolCallRequest {
            request_id: "a".into(),
            client_id: "c1".into(),
            tool: "terminal_list".into(),
            input: Value::Null,
        });
        hub.pending.push_back(ToolCallRequest {
            request_id: "b".into(),
            client_id: "c2".into(),
            tool: "terminal_list".into(),
            input: Value::Null,
        });
        let mine = hub.drain_for("c1");
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].request_id, "a");
        assert_eq!(hub.pending.len(), 1);
        assert!(hub.is_online("c1"));
        assert!(!hub.is_online("c2"));
    }
}
