//! Server-Sent Events 解码器，与项目内 anthropic.rs 先例对齐。

use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::Value;

use crate::types::StreamEvent;
use crate::{Error, Result};

/// 从 reqwest bytes stream 解析 SSE 事件流。
pub fn decode_sse_stream<S>(stream: S) -> impl Stream<Item = Result<StreamEvent>>
where
    S: Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Unpin,
{
    SseStream {
        inner: stream,
        buffer: String::new(),
    }
}

struct SseStream<S> {
    inner: S,
    buffer: String,
}

impl<S> Stream for SseStream<S>
where
    S: Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Unpin,
{
    type Item = Result<StreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(pos) = self.buffer.find("\n\n") {
                let raw = self.buffer[..pos].to_string();
                self.buffer.drain(..pos + 2);
                match parse_sse_event(&raw) {
                    Some(event) => return Poll::Ready(Some(Ok(event))),
                    None => continue,
                }
            }

            match self.inner.poll_next_unpin(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    self.buffer.push_str(&String::from_utf8_lossy(&chunk));
                }
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(Error::Http(e)))),
                Poll::Ready(None) => {
                    // 流结束但缓冲区可能还有未以 \n\n 结尾的事件。
                    if !self.buffer.trim().is_empty() {
                        let raw = self.buffer.clone();
                        self.buffer.clear();
                        if let Some(event) = parse_sse_event(&raw) {
                            return Poll::Ready(Some(Ok(event)));
                        }
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// 解析一段 SSE 文本（不含分隔符）。
///
/// 忽略无 `data:` 行的事件（心跳/注释）。若 JSON 无法解析则返回 `None`。
pub fn parse_sse_event(raw: &str) -> Option<StreamEvent> {
    let mut event_type = String::new();
    let mut data = String::new();

    for line in raw.lines() {
        if line.starts_with(':') {
            // ping / comment
            continue;
        }
        if let Some(val) = line.strip_prefix("event: ") {
            event_type = val.to_string();
        } else if let Some(val) = line.strip_prefix("data: ") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(val);
        }
    }

    if data.is_empty() {
        return None;
    }

    let parsed: Value = serde_json::from_str(&data).ok()?;

    // A2A SDK 常见形态：事件类型写在 `event:` 行，也可能嵌在 JSON 的 `kind` 字段。
    let event_name = if event_type.is_empty() {
        parsed.get("kind").and_then(|v| v.as_str()).unwrap_or("")
    } else {
        event_type.as_str()
    };

    match event_name {
        "task" => serde_json::from_value(parsed)
            .map(|task| StreamEvent::Task { task })
            .ok(),
        "taskStatusUpdate" | "statusUpdate" | "status" => serde_json::from_value(parsed)
            .map(|status_update| StreamEvent::StatusUpdate { status_update })
            .ok(),
        "taskArtifactUpdate" | "artifactUpdate" | "artifact" => serde_json::from_value(parsed)
            .map(|artifact_update| StreamEvent::ArtifactUpdate { artifact_update })
            .ok(),
        // 无显式事件类型：尝试按 tagged union 直接反序列化。
        _ => serde_json::from_value(parsed).ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Task, TaskState};

    #[test]
    fn test_parse_sse_status_update() {
        let raw = "event: taskStatusUpdate\ndata: {\"kind\":\"taskStatusUpdate\",\"taskId\":\"t1\",\"status\":{\"state\":\"working\"}}";
        let ev = parse_sse_event(raw).unwrap();
        match ev {
            StreamEvent::StatusUpdate { status_update } => {
                assert_eq!(status_update.task_id, "t1");
                assert!(matches!(status_update.status.state, TaskState::Working));
            }
            _ => panic!("unexpected"),
        }
    }

    #[test]
    fn test_parse_sse_artifact() {
        let raw = "event: taskArtifactUpdate\n\
                   data: {\"kind\":\"taskArtifactUpdate\",\"taskId\":\"t1\",\"artifact\":{\"artifactId\":\"a1\",\"parts\":[]}}";
        let ev = parse_sse_event(raw).unwrap();
        assert!(matches!(ev, StreamEvent::ArtifactUpdate { .. }));
    }

    #[test]
    fn test_ignore_ping() {
        let raw = ": ping\n\nevent: taskStatusUpdate\n\
                   data: {\"kind\":\"taskStatusUpdate\",\"taskId\":\"t1\",\"status\":{\"state\":\"working\"}}";
        let ev = parse_sse_event(raw).unwrap();
        assert!(matches!(ev, StreamEvent::StatusUpdate { .. }));
    }

    #[tokio::test]
    async fn test_decode_stream_from_string() {
        let payload = "event: task\ndata: {\"kind\":\"task\",\"id\":\"t1\",\"status\":{\"state\":\"working\"}}\n\n\
                       : ping\n\n\
                       event: taskStatusUpdate\ndata: {\"kind\":\"taskStatusUpdate\",\"taskId\":\"t1\",\"status\":{\"state\":\"completed\"},\"final\":true}\n\n";
        let stream = futures::stream::iter(vec![Ok::<_, reqwest::Error>(Bytes::from(payload))]);
        let mut decoded = decode_sse_stream(stream);

        let first = decoded.next().await.unwrap().unwrap();
        assert!(matches!(first, StreamEvent::Task { task: Task { .. } }));

        let second = decoded.next().await.unwrap().unwrap();
        assert!(matches!(
            second,
            StreamEvent::StatusUpdate { status_update } if matches!(status_update.status.state, TaskState::Completed)
        ));

        assert!(decoded.next().await.is_none());
    }
}
