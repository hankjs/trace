use std::collections::VecDeque;
use std::hash::{DefaultHasher, Hash, Hasher};
use serde_json::Value;

/// warning 阈值：无进展 streak ≥ 5 时注入 nudge（【SA 03】【AF 08】）
pub const LOOP_WARNING_THRESHOLD: usize = 5;
/// critical 阈值：无进展 streak ≥ 8（强制换路）
pub const LOOP_CRITICAL_THRESHOLD: usize = 8;
/// 全局熔断阈值：无进展 streak ≥ 10 时终止 run
pub const LOOP_BREAKER_THRESHOLD: usize = 10;
/// 滑动窗口大小（书中口径：30）
const WINDOW_SIZE: usize = 30;

/// 循环级别（按无进展 streak 分档）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopLevel {
    /// 无循环
    None,
    /// streak ≥ 5：注入 nudge
    Warning,
    /// streak ≥ 8：critical，强制换路
    Critical,
    /// streak ≥ 10：全局熔断，终止 run
    Breaker,
}

/// 一次工具调用记录：调用指纹 + 结果指纹（执行完成后回填）
struct CallRecord {
    tool_name: String,
    args_hash: u64,
    result_hash: Option<u64>,
}

/// Detects infinite loops in agent tool execution using a sliding window
/// of call records. 只有"相同调用 + 相同结果"才算无进展（【SA 03】【AF 08】）：
/// 参数相同但结果不同属正常探索，不计入 streak。
pub struct LoopDetector {
    window: VecDeque<CallRecord>,
}

impl LoopDetector {
    pub fn new() -> Self {
        Self {
            window: VecDeque::with_capacity(WINDOW_SIZE),
        }
    }

    /// 记录一次工具调用并返回当前循环级别。
    /// "无进展 streak" = 窗口尾部连续 N 条"调用指纹相同且结果指纹相同"的记录。
    pub fn record_and_check(&mut self, tool_name: &str, input: &Value) -> LoopLevel {
        if self.window.len() >= WINDOW_SIZE {
            self.window.pop_front();
        }
        self.window.push_back(CallRecord {
            tool_name: tool_name.to_string(),
            args_hash: Self::hash_json(input),
            result_hash: None,
        });
        self.level()
    }

    /// 工具执行完成后回填结果指纹：从窗口尾部找同 tool+argsHash
    /// 且尚未填结果的记录（【SA 03】resultHash 回填）。
    pub fn record_result(&mut self, tool_name: &str, input: &Value, result: &str) {
        let args_hash = Self::hash_json(input);
        let result_hash = Self::hash_str(result);
        for rec in self.window.iter_mut().rev() {
            if rec.tool_name == tool_name && rec.args_hash == args_hash && rec.result_hash.is_none()
            {
                rec.result_hash = Some(result_hash);
                return;
            }
        }
    }

    /// 当前循环级别（按无进展 streak 分档）
    pub fn level(&self) -> LoopLevel {
        let streak = self.no_progress_streak();
        if streak >= LOOP_BREAKER_THRESHOLD {
            LoopLevel::Breaker
        } else if streak >= LOOP_CRITICAL_THRESHOLD {
            LoopLevel::Critical
        } else if streak >= LOOP_WARNING_THRESHOLD {
            LoopLevel::Warning
        } else {
            LoopLevel::None
        }
    }

    /// 窗口尾部"相同调用 + 相同结果"的连续次数（无进展 streak）。
    /// 最近一次调用尚未执行（无结果指纹），以其之前的同指纹调用结果为基准。
    fn no_progress_streak(&self) -> usize {
        let Some(last) = self.window.back() else {
            return 0;
        };
        let mut streak = 1; // 当前调用本身
        let mut expected_result: Option<u64> = None;
        for rec in self.window.iter().rev().skip(1) {
            if rec.tool_name != last.tool_name || rec.args_hash != last.args_hash {
                break;
            }
            match (rec.result_hash, expected_result) {
                // 结果未回填（调用被中断/阻断），无法判定为无进展
                (None, _) => break,
                (Some(h), None) => {
                    expected_result = Some(h);
                    streak += 1;
                }
                (Some(h), Some(e)) if h == e => streak += 1,
                _ => break,
            }
        }
        streak
    }

    /// Get a string representation of the current detected loop pattern
    pub fn loop_pattern(&self) -> String {
        let Some(last) = self.window.back() else {
            return String::new();
        };
        let count = self
            .window
            .iter()
            .filter(|r| r.tool_name == last.tool_name && r.args_hash == last.args_hash)
            .count();
        format!(
            "{}:{:08x} (appears {} times)",
            last.tool_name,
            last.args_hash % 0xFFFFFFFF,
            count
        )
    }

    /// 滑动窗口大小（用于事件上报）
    pub fn window_size(&self) -> usize {
        WINDOW_SIZE
    }

    /// Reset the detector state（每轮新 user query 进入 loop 时调用）
    pub fn reset(&mut self) {
        self.window.clear();
    }

    fn hash_json(input: &Value) -> u64 {
        let mut hasher = DefaultHasher::new();
        input.to_string().hash(&mut hasher);
        hasher.finish()
    }

    fn hash_str(s: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        s.hash(&mut hasher);
        hasher.finish()
    }
}

impl Default for LoopDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 辅助：记录一次调用并回填结果
    fn record_call(detector: &mut LoopDetector, tool: &str, input: &Value, result: &str) -> LoopLevel {
        let level = detector.record_and_check(tool, input);
        detector.record_result(tool, input, result);
        level
    }

    #[test]
    fn test_same_args_different_results_no_streak() {
        let mut detector = LoopDetector::new();
        let input = serde_json::json!({"test": "value"});

        // 同参数但结果不同：属正常探索，不计 streak
        for i in 0..10 {
            let level = record_call(&mut detector, "tool1", &input, &format!("result-{i}"));
            assert_eq!(level, LoopLevel::None, "iteration {i}");
        }
    }

    #[test]
    fn test_same_args_same_result_thresholds() {
        let mut detector = LoopDetector::new();
        let input = serde_json::json!({"test": "value"});

        // 前 4 次：无告警
        for i in 1..=4 {
            let level = record_call(&mut detector, "tool1", &input, "same-result");
            assert_eq!(level, LoopLevel::None, "call {i}");
        }
        // 第 5 次：warning
        for i in 5..=7 {
            let level = record_call(&mut detector, "tool1", &input, "same-result");
            assert_eq!(level, LoopLevel::Warning, "call {i}");
        }
        // 第 8 次：critical
        for i in 8..=9 {
            let level = record_call(&mut detector, "tool1", &input, "same-result");
            assert_eq!(level, LoopLevel::Critical, "call {i}");
        }
        // 第 10 次：全局熔断
        let level = record_call(&mut detector, "tool1", &input, "same-result");
        assert_eq!(level, LoopLevel::Breaker);
    }

    #[test]
    fn test_result_not_backfilled_no_loop() {
        let mut detector = LoopDetector::new();
        let input = serde_json::json!({"test": "value"});

        // 只记录调用不回填结果：无法判定"相同结果"，不构成无进展
        for _ in 0..10 {
            assert_eq!(detector.record_and_check("tool1", &input), LoopLevel::None);
        }
    }

    #[test]
    fn test_streak_broken_by_different_result() {
        let mut detector = LoopDetector::new();
        let input = serde_json::json!({"test": "value"});

        for _ in 0..6 {
            record_call(&mut detector, "tool1", &input, "same-result");
        }
        // 第 7 次结果不同：回填后 streak 中断，下一次调用重新计数
        record_call(&mut detector, "tool1", &input, "different-result");
        let level = record_call(&mut detector, "tool1", &input, "different-result");
        assert_eq!(level, LoopLevel::None);
    }

    #[test]
    fn test_record_result_backfills_tail_record() {
        let mut detector = LoopDetector::new();
        let input = serde_json::json!({"test": "value"});

        // 连续两次相同调用后一次性回填：应回填最近一次（尾部）未填的记录
        detector.record_and_check("tool1", &input);
        detector.record_and_check("tool1", &input);
        detector.record_result("tool1", &input, "r");
        detector.record_result("tool1", &input, "r");
        // 再调两次相同调用并回填，streak 才累计到 4，仍低于 warning 阈值
        record_call(&mut detector, "tool1", &input, "r");
        let level = record_call(&mut detector, "tool1", &input, "r");
        assert_eq!(level, LoopLevel::None);
        let level = record_call(&mut detector, "tool1", &input, "r");
        assert_eq!(level, LoopLevel::Warning);
    }

    #[test]
    fn test_reset_clears_streak() {
        let mut detector = LoopDetector::new();
        let input = serde_json::json!({"test": "value"});

        for _ in 0..8 {
            record_call(&mut detector, "tool1", &input, "same-result");
        }
        detector.reset();
        let level = record_call(&mut detector, "tool1", &input, "same-result");
        assert_eq!(level, LoopLevel::None);
    }
}
