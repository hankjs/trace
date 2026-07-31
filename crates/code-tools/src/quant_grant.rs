use std::collections::HashMap;
use std::sync::Mutex;

/// 高成本 quant skill 的会话级授权存储。
///
/// 设计 §5.4.3：授权是会话态，不落表，重启即失效。
/// 每个 session 拥有一个剩余授权次数，高成本工具每次执行消耗 1；
/// `trial_batch` 按实际 patch 条数由调用方自行多次消耗（工具层不拆批）。
#[derive(Debug, Default)]
pub struct QuantGrantStore {
    inner: Mutex<HashMap<String, u32>>,
}

/// quant 高成本工具的待确认单（进程内存储，5 分钟 TTL，重启作废）。
///
/// 设计 §5.4.4：微信待确认单挂在 Orchestrator 进程内 map，key = 会话 id，
/// 不持久化、不跨会话、进程重启即全部作废。
#[derive(Debug, Clone, PartialEq)]
pub struct QuantPendingConfirm {
    pub tool_use_id: String,
    /// 待确认摘要（通常取 question 首行）。
    pub summary: String,
    pub question: String,
    pub options: Vec<String>,
    pub created_at_ms: i64,
    /// 触发来源，如 `weixin` / `trace_chat`。
    pub source: String,
}

/// quant 待确认单进程内存储。
///
/// 5 分钟 TTL，读取时惰性清理过期项；进程重启即全部作废。
#[derive(Debug, Default)]
pub struct QuantPendingConfirmStore {
    inner: Mutex<HashMap<String, QuantPendingConfirm>>,
}

impl QuantPendingConfirmStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 存入一条待确认单。
    pub fn insert(&self, session_id: &str, confirm: QuantPendingConfirm) {
        let mut guard = self.inner.lock().unwrap();
        guard.insert(session_id.to_string(), confirm);
    }

    /// 取出某会话的待确认单（存在时移除）。
    pub fn take(&self, session_id: &str) -> Option<QuantPendingConfirm> {
        let mut guard = self.inner.lock().unwrap();
        guard.remove(session_id)
    }

    /// 惰性清理超过 ttl_ms 的待确认单，返回清理条数。
    pub fn cleanup_expired(&self, ttl_ms: i64) -> usize {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let mut guard = self.inner.lock().unwrap();
        let before = guard.len();
        guard.retain(|_, v| now.saturating_sub(v.created_at_ms) <= ttl_ms);
        before.saturating_sub(guard.len())
    }

    /// 当前待确认单数量（仅用于测试/观测）。
    pub fn len(&self) -> usize {
        let guard = self.inner.lock().unwrap();
        guard.len()
    }
}

impl QuantGrantStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 查询某会话剩余授权次数。
    pub fn remaining(&self, session_id: &str) -> u32 {
        let guard = self.inner.lock().unwrap();
        guard.get(session_id).copied().unwrap_or(0)
    }

    /// 为某会话授予 N 次授权。返回授予后的剩余次数。
    pub fn grant(&self, session_id: &str, n: u32) -> u32 {
        let mut guard = self.inner.lock().unwrap();
        let entry = guard.entry(session_id.to_string()).or_insert(0);
        *entry = entry.saturating_add(n);
        *entry
    }

    /// 尝试消费一次授权。成功返回 true 并将剩余次数减一；无剩余返回 false。
    pub fn consume(&self, session_id: &str) -> bool {
        let mut guard = self.inner.lock().unwrap();
        match guard.get_mut(session_id) {
            Some(n) if *n > 0 => {
                *n -= 1;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grant_consume_session_isolation() {
        let store = QuantGrantStore::new();
        assert_eq!(store.remaining("s1"), 0);
        assert!(!store.consume("s1"));

        assert_eq!(store.grant("s1", 3), 3);
        assert_eq!(store.remaining("s1"), 3);
        assert!(store.consume("s1"));
        assert_eq!(store.remaining("s1"), 2);

        // s2 不受 s1 影响
        assert_eq!(store.remaining("s2"), 0);
        assert!(!store.consume("s2"));

        // 持续消费至 0
        assert!(store.consume("s1"));
        assert!(store.consume("s1"));
        assert_eq!(store.remaining("s1"), 0);
        assert!(!store.consume("s1"));
    }

    #[test]
    fn test_grant_accumulates() {
        let store = QuantGrantStore::new();
        store.grant("s", 2);
        store.grant("s", 3);
        assert_eq!(store.remaining("s"), 5);
    }

    #[test]
    fn test_grant_saturates() {
        let store = QuantGrantStore::new();
        store.grant("s", u32::MAX);
        store.grant("s", 1);
        assert_eq!(store.remaining("s"), u32::MAX);
    }

    #[test]
    fn test_pending_confirm_insert_take() {
        let store = QuantPendingConfirmStore::new();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let confirm = QuantPendingConfirm {
            tool_use_id: "tu_1".to_string(),
            summary: "summary".to_string(),
            question: "question".to_string(),
            options: vec!["确认".to_string()],
            created_at_ms: now,
            source: "weixin".to_string(),
        };
        store.insert("s1", confirm.clone());
        assert_eq!(store.len(), 1);

        let taken = store.take("s1").unwrap();
        assert_eq!(taken, confirm);
        assert!(store.take("s1").is_none());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_pending_confirm_session_isolation() {
        let store = QuantPendingConfirmStore::new();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        store.insert(
            "s1",
            QuantPendingConfirm {
                tool_use_id: "tu_1".to_string(),
                summary: "a".to_string(),
                question: "q".to_string(),
                options: vec![],
                created_at_ms: now,
                source: "weixin".to_string(),
            },
        );
        assert!(store.take("s2").is_none());
        assert!(store.take("s1").is_some());
    }

    #[test]
    fn test_pending_confirm_cleanup_expired() {
        let store = QuantPendingConfirmStore::new();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        store.insert(
            "expired",
            QuantPendingConfirm {
                tool_use_id: "tu_old".to_string(),
                summary: "old".to_string(),
                question: "q".to_string(),
                options: vec![],
                created_at_ms: now - 6 * 60 * 1000,
                source: "weixin".to_string(),
            },
        );
        store.insert(
            "fresh",
            QuantPendingConfirm {
                tool_use_id: "tu_new".to_string(),
                summary: "new".to_string(),
                question: "q".to_string(),
                options: vec![],
                created_at_ms: now,
                source: "weixin".to_string(),
            },
        );
        assert_eq!(store.cleanup_expired(5 * 60 * 1000), 1);
        assert!(store.take("expired").is_none());
        assert!(store.take("fresh").is_some());
    }

    #[test]
    fn test_pending_confirm_restart_loses_all() {
        // 进程重启即作废：新 store 实例不含任何待确认单。
        let old_store = QuantPendingConfirmStore::new();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        old_store.insert(
            "s1",
            QuantPendingConfirm {
                tool_use_id: "tu_1".to_string(),
                summary: "a".to_string(),
                question: "q".to_string(),
                options: vec![],
                created_at_ms: now,
                source: "weixin".to_string(),
            },
        );
        drop(old_store);

        let new_store = QuantPendingConfirmStore::new();
        assert!(new_store.take("s1").is_none());
    }
}
