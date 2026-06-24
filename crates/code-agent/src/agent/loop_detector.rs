use std::collections::VecDeque;
use std::hash::{DefaultHasher, Hash, Hasher};
use serde_json::Value;

/// Detects infinite loops in agent tool execution using a sliding window
/// of fingerprints. A loop is detected when the same fingerprint appears
/// more than `repeat_threshold` times within the window.
pub struct LoopDetector {
    window: VecDeque<String>,
    window_size: usize,
    repeat_threshold: usize,
    consecutive_loops: usize,
}

impl LoopDetector {
    /// Create a new LoopDetector with default window_size=6 and repeat_threshold=2
    pub fn new() -> Self {
        Self {
            window: VecDeque::with_capacity(6),
            window_size: 6,
            repeat_threshold: 2,
            consecutive_loops: 0,
        }
    }

    /// Record a tool execution and check if a loop is detected.
    /// Automatically increments consecutive_loops counter on detection.
    /// Returns true if loop detected, false otherwise.
    pub fn record_and_check(&mut self, tool_name: &str, input: &Value) -> bool {
        let fingerprint = Self::fingerprint(tool_name, input);

        // Add to window
        if self.window.len() >= self.window_size {
            self.window.pop_front();
        }
        self.window.push_back(fingerprint);

        // Check if loop detected
        if self.detect_loop() {
            self.consecutive_loops += 1;
            true
        } else {
            self.consecutive_loops = 0;
            false
        }
    }

    /// Legacy API — same as record_and_check
    pub fn record(&mut self, tool_name: &str, input: &Value) -> bool {
        self.record_and_check(tool_name, input)
    }

    /// Check if the loop count has reached the termination threshold.
    /// 同时检查窗口内独特调用比率，防止单次不同调用绕过连续计数（P2）。
    pub fn should_terminate(&self, threshold: usize) -> bool {
        if self.consecutive_loops >= threshold {
            return true;
        }
        // 窗口足够大时，若独特调用比率 <30% 也触发终止
        if self.window.len() >= 4 {
            let unique_count = self.window.iter().collect::<std::collections::HashSet<_>>().len();
            (unique_count as f64 / self.window.len() as f64) < 0.3
        } else {
            false
        }
    }

    /// Get a string representation of the current detected loop pattern
    pub fn loop_pattern(&self) -> String {
        if self.window.is_empty() {
            return String::new();
        }

        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for fp in &self.window {
            *counts.entry(fp.as_str()).or_insert(0) += 1;
        }

        if let Some((pattern, count)) = counts.iter().max_by_key(|&(_, &c)| c) {
            format!("{} (appears {} times)", pattern, count)
        } else {
            String::new()
        }
    }

    /// Reset the detector state
    pub fn reset(&mut self) {
        self.window.clear();
        self.consecutive_loops = 0;
    }

    /// Generate a fingerprint for a tool invocation: "name:hash8"
    fn fingerprint(tool_name: &str, input: &Value) -> String {
        let input_str = input.to_string();
        let mut hasher = DefaultHasher::new();
        input_str.hash(&mut hasher);
        let hash = hasher.finish();
        let hash_short = format!("{:08x}", hash % 0xFFFFFFFF);
        format!("{}:{}", tool_name, hash_short)
    }

    /// Check if a loop is detected in the current window.
    fn detect_loop(&self) -> bool {
        if self.window.len() < self.repeat_threshold {
            return false;
        }

        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for fp in &self.window {
            *counts.entry(fp.as_str()).or_insert(0) += 1;
        }

        // 策略1: 单指纹重复次数超过阈值
        if counts.values().any(|&count| count >= self.repeat_threshold) {
            return true;
        }

        // 策略2: 窗口内重复率 >70%
        if self.window.len() >= 4 {
            let unique_count = counts.len();
            let total = self.window.len();
            let unique_ratio = unique_count as f64 / total as f64;
            if unique_ratio < 0.3 {
                return true;
            }
        }

        false
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

    #[test]
    fn test_loop_detection() {
        let mut detector = LoopDetector::new();
        let input = serde_json::json!({"test": "value"});

        // First call: no loop yet
        assert!(!detector.record_and_check("tool1", &input));
        // Second identical call: loop detected (consecutive_loops=1)
        assert!(detector.record_and_check("tool1", &input));
        assert!(!detector.should_terminate(2));
        // Third identical call: loop detected again (consecutive_loops=2)
        assert!(detector.record_and_check("tool1", &input));
        assert!(detector.should_terminate(2));
    }

    #[test]
    fn test_different_tools_no_loop() {
        let mut detector = LoopDetector::new();
        let input = serde_json::json!({"test": "value"});

        assert!(!detector.record_and_check("tool1", &input));
        assert!(!detector.record_and_check("tool2", &input));
        assert!(!detector.record_and_check("tool3", &input));
        assert!(!detector.should_terminate(3));
    }

    #[test]
    fn test_consecutive_loops_reset() {
        let mut detector = LoopDetector::new();
        let input1 = serde_json::json!({"test": "1"});
        let input2 = serde_json::json!({"test": "2"});
        let input3 = serde_json::json!({"test": "3"});

        // Trigger loop
        detector.record_and_check("tool1", &input1);
        assert!(detector.record_and_check("tool1", &input1));

        // Different tools should reset counter
        detector.record_and_check("tool2", &input2);
        detector.record_and_check("tool3", &input3);
        // After enough unique calls, consecutive_loops resets
    }
}
