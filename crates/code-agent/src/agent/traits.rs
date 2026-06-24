use serde::{Deserialize, Serialize};

/// Strategy for when the Orchestrator should invoke a Think phase
#[derive(Debug, Clone, Default)]
pub enum ThinkStrategy {
    /// Think every iteration
    Always,
    /// Think only when conditions warrant it (default)
    #[default]
    Conditional,
    /// Never think — degrades to flat loop
    Never,
}

/// Concurrency policy for worker execution
#[derive(Debug, Clone, Default)]
pub enum ConcurrencyPolicy {
    /// All workers run in parallel (read-only tasks)
    Parallel,
    /// Workers run sequentially (write operations that may conflict)
    #[default]
    Sequential,
}

/// A task delegated from Orchestrator to Worker
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegatedTask {
    pub id: String,
    pub description: String,
    pub context: String,
    pub tools_allowed: Vec<String>,
    /// Paths this task may affect (for conflict detection)
    #[serde(default)]
    pub affected_paths: Vec<String>,
}

/// Status of a completed worker task
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Success,
    Failed,
    NeedsHumanInput,
}

/// Artifact produced by a worker (file change, command output, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub kind: String,
    pub description: String,
    pub content: String,
}

/// Result returned by a Worker to the Orchestrator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub task_id: String,
    pub status: TaskStatus,
    /// Compressed summary injected into Orchestrator context
    pub summary: String,
    /// Structured artifacts (file changes, outputs)
    pub artifacts: Vec<Artifact>,
    /// 子任务执行期间产生的文件变更（FR-PERM-6，回填父 run_state）
    #[serde(default)]
    pub file_changes: Vec<crate::types::FileChange>,
    /// 子任务执行期间的权限拒绝（FR-PERM-6，回填父 run_state）
    #[serde(default)]
    pub permission_denials: Vec<String>,
}

/// Verdict from the Verifier
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Approved,
    NeedsRevision,
    Rejected,
}

/// Result of verification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub verdict: Verdict,
    pub issues: Vec<String>,
}
