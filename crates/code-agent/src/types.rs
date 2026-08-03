use crate::agent::{TaskStatus, Verdict};
use hank_provider::{ContentBlock, Message, StopReason, ToolDefinition};
use serde::{Deserialize, Serialize};

/// 文件变更类型
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Add,
    Update,
    Delete,
}

/// 单个文件变更记录（用于 file.changed 事件与 artifact 索引）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub kind: FileChangeKind,
}

/// run 的终态状态
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Success,
    Failed,
    Cancelled,
}

/// Agent 提议的后续动作（由渠道渲染为可点按钮）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestedAction {
    pub label: String,
    pub prompt: String,
}

/// ask_user 多问题中的单题
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskUserQuestion {
    pub id: String,
    pub question: String,
    pub options: Vec<String>,
}

/// Events emitted by the agent loop to the caller (SSE stream)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    TextDelta {
        text: String,
    },
    ToolStart {
        id: String,
        name: String,
        input: String,
        /// 关联到完整 run / turn / LLM 调用，旧的外部执行事件可为空。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        risk: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
    },
    ToolResult {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        content: String,
        is_error: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
    },
    TurnComplete,
    Error {
        message: String,
    },
    /// Orchestrator Think phase output
    Thinking {
        text: String,
    },
    /// A worker agent was spawned
    WorkerSpawned {
        task_id: String,
        description: String,
    },
    /// A worker agent completed
    WorkerCompleted {
        task_id: String,
        status: TaskStatus,
        summary: String,
    },
    /// Verification result from the Verifier
    Verification {
        verdict: Verdict,
        issues: Vec<String>,
    },
    /// Loop detected in agent execution
    LoopDetected {
        pattern: String,
        window_size: usize,
    },
    /// Token budget warning
    TokenWarning {
        used_tokens: usize,
        total_budget: usize,
        percent: u8,
        action: String,
    },
    /// Compression triggered
    CompressionTriggered {
        before_tokens: usize,
        after_tokens: usize,
        strategy: String,
    },
    /// LLM call metrics (token usage + latency)
    Metrics {
        input_tokens: u32,
        output_tokens: u32,
        latency_ms: u64,
        model: String,
        provider: String,
        phase: Option<String>,
    },
    /// Tool execution metrics
    ToolMetrics {
        tool_name: String,
        duration_ms: u64,
        is_error: bool,
    },
    /// Provider fallback occurred during chat
    ProviderFallback {
        from: String,
        to: String,
        reason: String,
    },
    /// A spec was updated by agent tool
    SpecUpdated {
        spec_id: String,
        capability: String,
        version: i32,
    },
    /// A task status was updated by agent tool
    TaskUpdated {
        task_id: String,
        change_id: String,
        status: String,
    },
    /// An artifact was updated by agent tool
    ArtifactUpdated {
        artifact_id: String,
        change_id: String,
        #[serde(rename = "artifact_type")]
        artifact_type: String,
    },
    /// Agent is asking the user a question (interrupts agent loop)
    AskUser {
        question: String,
        options: Vec<String>,
        tool_use_id: String,
        /// 可选标记，用于区分普通 ask_user 与 quant 高成本确认等场景。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        /// 多问题作答。为空表示单问题（走 question / options）。
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        questions: Vec<AskUserQuestion>,
    },
    /// Agent 提议的后续动作（不中断循环，由渠道渲染成按钮）
    SuggestedActions {
        actions: Vec<SuggestedAction>,
    },
    /// Explore phase completed for a change
    ExploreComplete {
        change_id: String,
        summary: String,
    },
    /// Generate phase completed for a change
    GenerateComplete {
        change_id: String,
        artifact_count: u32,
    },
    /// 每次 LLM 调用前 emit，记录请求参数
    LlmRequest {
        call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        model: String,
        provider: String,
        system: Option<String>,
        /// 实际发送给 provider 的完整上下文，不是摘要或预览。
        messages: Vec<Message>,
        tools: Vec<String>,
        /// 完整工具描述与 JSON Schema，便于复现模型当时看到的工具面。
        tool_definitions: Vec<ToolDefinition>,
        max_tokens: u32,
        message_count: usize,
        phase: String,
    },
    /// 每次 LLM 流完成后 emit，和 LlmRequest.call_id 一一对应。
    LlmResponse {
        call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        model: String,
        provider: String,
        phase: String,
        content: Vec<ContentBlock>,
        stop_reason: StopReason,
        input_tokens: u32,
        output_tokens: u32,
        cache_read_tokens: u32,
        cache_write_tokens: u32,
        latency_ms: u64,
        cancelled: bool,
        timed_out: bool,
    },
    /// LLM 请求建立或流消费阶段的可重试失败。
    LlmRetry {
        call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        model: String,
        provider: String,
        phase: String,
        stage: String,
        failed_attempt: u32,
        next_attempt: u32,
        delay_ms: u64,
        error: String,
    },
    /// 一次 LLM 调用在所有重试后仍失败。
    LlmFailed {
        call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        model: String,
        provider: String,
        phase: String,
        stage: String,
        attempt: u32,
        retryable: bool,
        error: String,
    },
    /// Streaming tool output delta (实时输出)
    ToolOutputDelta {
        id: String,
        chunk: String,
    },

    // ─── Run / Turn 生命周期事件 (FR-EVT-2/3, FR-LOOP-7) ───
    /// 一次完整运行开始
    RunStarted {
        run_id: String,
        timestamp: String,
        cwd: Option<String>,
        model: String,
        permission_mode: String,
        tools: Vec<String>,
    },
    /// 一次完整运行完成
    RunCompleted {
        run_id: String,
        timestamp: String,
        status: RunStatus,
        input_tokens: u32,
        output_tokens: u32,
        summary: String,
        permission_denials: Vec<String>,
        file_changes: Vec<FileChange>,
    },
    /// 一次完整运行失败
    RunFailed {
        run_id: String,
        timestamp: String,
        message: String,
    },
    /// 一次完整运行被取消
    RunCancelled {
        run_id: String,
        timestamp: String,
        /// FR-SESSION-5: 取消前已完成的文件变更
        #[serde(default)]
        file_changes: Vec<FileChange>,
        /// FR-SESSION-5: 取消前已记录的权限拒绝
        #[serde(default)]
        permission_denials: Vec<String>,
    },
    /// 一轮 LLM 交互开始
    TurnStarted {
        run_id: String,
        turn_id: String,
        timestamp: String,
        phase: String,
        message_count: usize,
    },
    /// 一轮 LLM 交互完成
    TurnCompleted {
        run_id: String,
        turn_id: String,
        timestamp: String,
    },

    // ─── 结构化文件变更事件 (FR-EVT-4, FR-TOOL-6) ───
    /// 编辑类工具产生的结构化文件变更
    FileChanged {
        run_id: String,
        turn_id: String,
        changes: Vec<FileChange>,
    },

    // ─── 权限事件 (FR-PERM-5/6, FR-EVT-9) ───
    /// 工具执行需要审批
    PermissionRequested {
        run_id: String,
        turn_id: String,
        tool: String,
        tool_use_id: String,
        risk: String,
        reason: String,
    },
    /// 工具执行被拒绝
    PermissionDenied {
        run_id: String,
        turn_id: String,
        tool: String,
        tool_use_id: String,
        reason: String,
    },

    // ─── 上下文组装事件 (FR-CTX-9, FR-EVT-9) ───
    /// 上下文按分层组装完成（debug 摘要，不含完整 system prompt）
    ContextAssembled {
        run_id: String,
        turn_id: String,
        segments: Vec<String>,
        total_chars: usize,
    },

    // ─── 计划事件 (FR-EVT-9) ───
    /// 用户可见的简短计划/状态更新
    PlanUpdated {
        run_id: String,
        text: String,
    },

    // ─── 验证事件 (FR-VERIFY-2, FR-EVT-9) ───
    /// 验证阶段开始
    VerificationStarted {
        run_id: String,
        command: Option<String>,
    },
    /// 验证阶段完成
    VerificationCompleted {
        run_id: String,
        verdict: Verdict,
        issues: Vec<String>,
    },
}
