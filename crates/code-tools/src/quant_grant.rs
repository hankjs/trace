use std::collections::HashMap;
use std::sync::Mutex;

/// 高成本 quant skill 的会话级授权存储。
///
/// 设计 §5.4.3：授权是会话态，不落表，重启即失效。
/// 每个 session 拥有一个剩余授权次数，高成本工具每次执行消耗 1；
/// `trial_batch` 按实际 patch 条数由调用方自行多次消耗（工具层不拆批）。
///
/// 注意：待确认单已迁到 `agent_interactions` 表（见 hank-db），不再用进程内 map。
/// 授权计数仍是进程态——确认后的 N 次消费额度随重启清零，符合设计。
#[derive(Debug, Default)]
pub struct QuantGrantStore {
    inner: Mutex<HashMap<String, u32>>,
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
}
