//! 单任务闸门 + 实时进度快照。
//!
//! 两个职责，都是为了让渠道（飞书/微信）在"同一会话同时只跑一个任务"的前提下，
//! 还能随时回答"进度怎样了"：
//!
//! - `try_acquire`：派发名额的原子抢占。`active_tasks` 要等 run_chat_turn 走完
//!   工作区准备 / 鉴权 / git link 才登记，中间有秒级空窗；仅检查 `active_tasks`
//!   会让空窗期到达的第二条消息也通过闸门，起第二个并发 run。这里先抢名额，
//!   拿到 guard 才允许派发，guard 活到 run_chat_turn 返回（此时已登记 active_tasks）。
//! - `set_progress` / `progress`：pusher 把当前百分比与最近活动写进来，
//!   渠道收到进度询问时直接读，不必等卡片刷新。
//!
//! 后续要支持"多任务并发"时，把 key 从 session_id 换成 task_id、
//! 并允许同会话多个名额即可，调用方语义不用变。

use crate::chat::EventEntry;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, RwLock};

/// 一个正在执行的任务的对外可见状态。
#[derive(Clone, Debug)]
pub struct ProgressSnapshot {
    /// 估算进度，运行中封顶 90%
    pub percent: u32,
    /// 当前动作，如"调用工具 Bash"
    pub detail: String,
    /// 最近若干条活动，新的在后
    pub activities: Vec<String>,
    /// 任务开始时间，用于算已用时
    pub started_at: Instant,
}

impl ProgressSnapshot {
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }
}

#[derive(Default)]
pub struct TaskRegistry {
    progress: RwLock<HashMap<String, ProgressSnapshot>>,
    /// 已抢到派发名额、但还没登记进 active_tasks 的会话
    dispatching: RwLock<HashSet<String>>,
}

impl TaskRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 抢占派发名额。已有在途派发时返回 None，调用方应改为回进度而不是再起一轮。
    pub async fn try_acquire(self: &Arc<Self>, session_id: &str) -> Option<DispatchGuard> {
        let mut dispatching = self.dispatching.write().await;
        if !dispatching.insert(session_id.to_string()) {
            return None;
        }
        Some(DispatchGuard {
            registry: Arc::clone(self),
            session_id: session_id.to_string(),
            released: false,
        })
    }

    /// 是否有在途派发（还没来得及登记 active_tasks 的那一小段）。
    pub async fn is_dispatching(&self, session_id: &str) -> bool {
        self.dispatching.read().await.contains(session_id)
    }

    pub async fn set_progress(&self, session_id: &str, snapshot: ProgressSnapshot) {
        self.progress
            .write()
            .await
            .insert(session_id.to_string(), snapshot);
    }

    pub async fn progress(&self, session_id: &str) -> Option<ProgressSnapshot> {
        self.progress.read().await.get(session_id).cloned()
    }

    /// 任务收尾：清掉进度，避免后续询问读到已结束任务的残留状态。
    pub async fn clear_progress(&self, session_id: &str) {
        self.progress.write().await.remove(session_id);
    }

    async fn release(&self, session_id: &str) {
        self.dispatching.write().await.remove(session_id);
    }
}

/// 派发名额的 RAII 凭证：drop 即释放，保证 early return / `?` 不会漏掉名额。
pub struct DispatchGuard {
    registry: Arc<TaskRegistry>,
    session_id: String,
    released: bool,
}

impl DispatchGuard {
    /// 显式释放（能 await，比 Drop 里补救更确定）。
    pub async fn release(mut self) {
        self.released = true;
        self.registry.release(&self.session_id).await;
    }
}

impl Drop for DispatchGuard {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        // 兜底：调用方提前 return 时也要还名额，否则该会话再也派发不出去。
        let registry = Arc::clone(&self.registry);
        let session_id = std::mem::take(&mut self.session_id);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                registry.release(&session_id).await;
            });
        }
    }
}

/// 把已用时渲染成中文短描述（"3 分 12 秒"）。
pub fn format_elapsed(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs} 秒")
    } else if secs < 3600 {
        format!("{} 分 {} 秒", secs / 60, secs % 60)
    } else {
        format!("{} 小时 {} 分", secs / 3600, (secs % 3600) / 60)
    }
}

// ── 事件流补偿读取 ──

/// 等事件的轮询间隔。到点没等到事件就回头看 EventBuffer 是否已收尾，
/// 避免终态事件被丢掉后渠道侧永远停在"运行中"。
pub const RECV_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// 一次 recv 的结果。
///
/// broadcast 容量有限（256），Claude Code 这类高频后端很容易把订阅者挤成
/// Lagged，被丢掉的那批里可能正好有 RunCompleted。而 EventBuffer 常驻
/// `state.event_buffers`、sender 不会 drop，所以 recv 也几乎不会返回 Closed
/// ——只等 broadcast 会死等。两种情况都必须回 EventBuffer 补齐。
// 变体大小差异是有意的：Event 携带完整事件，其余是控制信号。
// 这个值只在 recv 处即刻消费掉，不进集合，装箱反而多一次分配。
#[allow(clippy::large_enum_variant)]
pub enum Incoming {
    /// 正常拿到事件
    Event(EventEntry),
    /// 滞后丢了 n 条，需要按 id 从 EventBuffer 补读
    Lagged(u64),
    /// 一段时间没有新事件，回头确认 buffer 是否已收尾
    Idle,
    /// 发送端关闭
    Closed,
}

/// 带超时地取下一个事件，超时返回 `Idle` 让调用方去查 buffer。
pub async fn next_event(rx: &mut broadcast::Receiver<EventEntry>) -> Incoming {
    match tokio::time::timeout(RECV_POLL_INTERVAL, rx.recv()).await {
        Ok(Ok(entry)) => Incoming::Event(entry),
        Ok(Err(broadcast::error::RecvError::Lagged(n))) => Incoming::Lagged(n),
        Ok(Err(broadcast::error::RecvError::Closed)) => Incoming::Closed,
        Err(_) => Incoming::Idle,
    }
}

/// 从 EventBuffer 补读 id > last_id 的事件，并回报该 buffer 是否已标记完成。
///
/// EventBuffer 保存了本轮全部事件，是滞后后唯一可靠的补偿来源。
/// buffer 不存在（例如新一轮任务重建）时按已结束处理，避免死等。
pub async fn drain_buffer(
    state: &Arc<crate::AppState>,
    session_id: &str,
    last_id: u64,
) -> (VecDeque<EventEntry>, bool) {
    let buffers = state.event_buffers.read().await;
    events_after(buffers.get(session_id), last_id)
}

/// drain_buffer 的纯函数内核（脱离 AppState 便于单测）。
pub fn events_after(
    buffer: Option<&crate::chat::EventBuffer>,
    last_id: u64,
) -> (VecDeque<EventEntry>, bool) {
    match buffer {
        Some(buffer) => (
            buffer
                .events
                .iter()
                .filter(|entry| entry.id > last_id)
                .cloned()
                .collect(),
            buffer.completed,
        ),
        None => (VecDeque::new(), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dispatch_slot_is_exclusive_until_released() {
        let registry = Arc::new(TaskRegistry::new());
        let first = registry.try_acquire("s1").await;
        assert!(first.is_some(), "首个派发应拿到名额");
        assert!(
            registry.try_acquire("s1").await.is_none(),
            "同会话第二次派发必须被拒绝"
        );
        assert!(
            registry.try_acquire("s2").await.is_some(),
            "不同会话互不影响"
        );

        first.unwrap().release().await;
        assert!(
            registry.try_acquire("s1").await.is_some(),
            "释放后应能再次派发"
        );
    }

    #[tokio::test]
    async fn drop_returns_the_slot() {
        let registry = Arc::new(TaskRegistry::new());
        drop(registry.try_acquire("s1").await.unwrap());
        // Drop 里是 spawn 释放，让出执行权等它跑完
        tokio::task::yield_now().await;
        assert!(!registry.is_dispatching("s1").await);
    }

    #[tokio::test]
    async fn progress_round_trips_and_clears() {
        let registry = Arc::new(TaskRegistry::new());
        registry
            .set_progress(
                "s1",
                ProgressSnapshot {
                    percent: 60,
                    detail: "调用工具 Bash".into(),
                    activities: vec!["调用工具 Bash".into()],
                    started_at: Instant::now(),
                },
            )
            .await;
        assert_eq!(registry.progress("s1").await.unwrap().percent, 60);
        registry.clear_progress("s1").await;
        assert!(registry.progress("s1").await.is_none());
    }

    #[test]
    fn elapsed_formats_by_magnitude() {
        assert_eq!(format_elapsed(Duration::from_secs(42)), "42 秒");
        assert_eq!(format_elapsed(Duration::from_secs(192)), "3 分 12 秒");
        assert_eq!(format_elapsed(Duration::from_secs(7_260)), "2 小时 1 分");
    }

    // ── 事件补偿：这是"任务跑完不汇报、卡片停在 90%"的根因所在 ──

    fn buffer_with(events: usize, completed: bool) -> crate::chat::EventBuffer {
        let mut buffer = crate::chat::EventBuffer::new();
        for _ in 0..events {
            buffer.push(code_agent::AgentEvent::TurnComplete);
        }
        buffer.completed = completed;
        buffer
    }

    #[test]
    fn recovers_only_events_after_last_seen_id() {
        let buffer = buffer_with(5, false);
        let (missed, completed) = events_after(Some(&buffer), 2);
        assert_eq!(
            missed.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![3, 4, 5],
            "只应补读没见过的事件，已处理过的不能重放"
        );
        assert!(!completed);
    }

    #[test]
    fn recovers_terminal_event_dropped_by_lagging_broadcast() {
        // 复现 bug：终态事件在 broadcast 里被挤掉，订阅者只见过前 2 条。
        // EventBuffer 仍留着全部事件，补读必须能捞回终态事件。
        let mut buffer = crate::chat::EventBuffer::new();
        buffer.push(code_agent::AgentEvent::TurnStarted {
            run_id: "r1".into(),
            turn_id: "t1".into(),
            timestamp: String::new(),
            phase: String::new(),
            message_count: 0,
        });
        buffer.push(code_agent::AgentEvent::TextDelta { text: "hi".into() });
        buffer.push(code_agent::AgentEvent::RunFailed {
            run_id: "r1".into(),
            timestamp: String::new(),
            message: "boom".into(),
        });
        buffer.completed = true;

        let (missed, completed) = events_after(Some(&buffer), 2);
        assert!(completed);
        assert!(
            missed
                .iter()
                .any(|e| matches!(e.event, code_agent::AgentEvent::RunFailed { .. })),
            "滞后丢掉的终态事件必须能从 buffer 补回来"
        );
    }

    #[test]
    fn exhausted_completed_buffer_reports_done() {
        // 已收尾且没有新事件 → 调用方据此走兜底收尾，而不是继续等下去
        let buffer = buffer_with(3, true);
        let (missed, completed) = events_after(Some(&buffer), 3);
        assert!(missed.is_empty());
        assert!(completed);
    }

    #[test]
    fn missing_buffer_is_treated_as_finished() {
        // buffer 被新一轮任务换掉：必须当作结束，否则 pusher 永远等不到事件
        let (missed, completed) = events_after(None, 0);
        assert!(missed.is_empty());
        assert!(completed, "buffer 不存在时不能让调用方死等");
    }
}
