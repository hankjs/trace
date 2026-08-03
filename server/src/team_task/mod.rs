//! 团队任务流水线：开发 → 评审 → 测试的多角色编排。
//!
//! 本模块只放**纯函数与类型**，不做任何 IO：
//! - `decide_next`：状态机唯一判定点。分支多，必须能单测；走 DB 测既慢又覆盖不全。
//! - `parse_handoff`：从角色输出正文里提取结构化交接产物。
//!
//! 派发 run、发卡片、读写 DB 都在后续的 `orchestrator` 里，不要写进这里。
//!
pub mod card;
pub mod orchestrator;
pub mod roles;
pub mod routes;
pub mod settings;

use hank_db::TeamTaskSettings;

// ---------------------------------------------------------------------------
// 角色注册表
// ---------------------------------------------------------------------------

/// 角色定义。加第四个角色（如「文档」）只需往 ROLE_DEFS 加一行 + 写 prompt 函数，
/// 流转顺序由配置的 roles 数组顺序决定。
pub struct RoleDef {
    pub id: &'static str,
    /// 卡片与看板展示用中文名
    pub label: &'static str,
    /// 该角色运行中对应的 team_tasks.status
    pub running_status: &'static str,
    /// 该角色是否要求结构化 verdict（评审/测试要，开发不要）
    pub needs_verdict: bool,
    /// 该角色的 prompt 构造函数。函数指针而非 Box<dyn Fn>，
    /// 因为 ROLE_DEFS 是 const，且 prompt 构造是纯函数无需捕获环境。
    pub prompt: fn(&roles::RolePromptInput<'_>) -> String,
}

pub const ROLE_DEFS: &[RoleDef] = &[
    RoleDef {
        id: "developer",
        label: "开发",
        running_status: "running_developer",
        needs_verdict: false,
        prompt: roles::developer_prompt,
    },
    RoleDef {
        id: "reviewer",
        label: "评审",
        running_status: "running_reviewer",
        needs_verdict: true,
        prompt: roles::reviewer_prompt,
    },
    RoleDef {
        id: "tester",
        label: "测试",
        running_status: "running_tester",
        needs_verdict: true,
        prompt: roles::tester_prompt,
    },
];

/// 按 id 查角色定义；未知 id 返回 None（调用方转成用户可见错误，不 panic）。
pub fn role_def(id: &str) -> Option<&'static RoleDef> {
    ROLE_DEFS.iter().find(|r| r.id == id)
}

/// 按角色 id 构造 prompt；未知角色返回 None（调用方转用户可见错误，不 panic）。
pub fn role_prompt(role: &str, input: &roles::RolePromptInput<'_>) -> Option<String> {
    role_def(role).map(|d| (d.prompt)(input))
}

/// 按配置的 roles 顺序取下一个角色；已是最后一个返回 None。
/// 注意用**配置顺序**而非 ROLE_DEFS 顺序——配置可裁剪成 ["developer"]。
pub fn next_role(roles: &[String], current: &str) -> Option<String> {
    let idx = roles.iter().position(|r| r == current)?;
    roles.get(idx + 1).cloned()
}

/// 配置里的第一个角色（流水线入口）。roles 为空返回 None。
pub fn first_role(roles: &[String]) -> Option<String> {
    roles.first().cloned()
}

// ---------------------------------------------------------------------------
// 状态常量
// 用 &'static str 常量而非枚举——这些值要在 DB、卡片、REST 三处流转，
// 枚举会在每个边界多一次转换。
// ---------------------------------------------------------------------------

pub const STATUS_PENDING_CONFIRM: &str = "pending_confirm";
pub const STATUS_PENDING_REVIEW_GATE: &str = "pending_review_gate";
pub const STATUS_PENDING_DEV_GATE: &str = "pending_dev_gate";
pub const STATUS_PENDING_TEST_GATE: &str = "pending_test_gate";
pub const STATUS_DONE: &str = "done";
pub const STATUS_FAILED: &str = "failed";
pub const STATUS_CANCELLED: &str = "cancelled";

/// 是否终态。终态任务收到任何 Trigger 都应 Ignore。
pub fn is_terminal(status: &str) -> bool {
    matches!(status, STATUS_DONE | STATUS_FAILED | STATUS_CANCELLED)
}

/// 是否某角色运行中（status 形如 running_*）。
pub fn is_running(status: &str) -> bool {
    status.starts_with("running_")
}

// ---------------------------------------------------------------------------
// Verdict
// ---------------------------------------------------------------------------

/// 角色自评结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Reject,
    /// 模型输出没解析出结论。**不是**「需要人工确认」——见 decide_next 的处理。
    Unknown,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Reject => "reject",
            Verdict::Unknown => "unknown",
        }
    }

    /// 宽松解析：大小写不敏感，容忍首尾空白与结尾标点（句号/逗号/分号，全角半角）。
    /// 识别 pass/通过/approved 与 reject/打回/不通过/rejected；其余一律 Unknown。
    pub fn parse(raw: &str) -> Self {
        let s = raw.trim();
        let s = s.trim_end_matches(['.', ',', ';', '。', '，', '；', '!', '！', '?', '？']);
        let s = s.trim();
        // 中文关键词 to_ascii_lowercase 后原样保留，可与英文一起 match
        match s.to_ascii_lowercase().as_str() {
            "pass" | "通过" | "approved" => Verdict::Pass,
            "reject" | "打回" | "不通过" | "rejected" => Verdict::Reject,
            _ => Verdict::Unknown,
        }
    }
}

// ---------------------------------------------------------------------------
// decide_next 类型
// ---------------------------------------------------------------------------

/// decide_next 的输入快照（字段多，避免位置参数）。
#[derive(Debug, Clone)]
pub struct DecideInput<'a> {
    pub status: &'a str,
    /// 当前角色；终态或待确认时可为 None
    pub current_role: Option<&'a str>,
    /// 已用开发轮次（team_tasks.dev_rounds）
    pub dev_rounds: i32,
    pub trigger: Trigger<'a>,
}

#[derive(Debug, Clone)]
pub enum Trigger<'a> {
    /// 闸门被应答（飞书按钮 / admin 手动应答共用）
    GateAnswered { answer: &'a str },
    /// 某角色 run 走到终态
    RunFinished {
        role: &'a str,
        /// 该角色本轮轮次（编排器透传审计；状态机派发轮次写在 Decision 里，不读此字段）
        #[allow(dead_code)]
        round: i32,
        outcome: RunOutcome,
    },
    /// 看板或飞书 /stop 取消
    Cancelled { operator: &'a str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// run 正常结束，带角色自评结论（无 verdict 的角色传 Pass）
    Finished(Verdict),
    /// run 本身失败（节点离线、超时、CLI 报错）
    Failed,
}

/// 状态机判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// 派发某角色的某一轮
    DispatchRole { role: String, round: i32 },
    /// 开人工闸门
    OpenGate { boundary: GateBoundary },
    /// 走终态
    Finish {
        status: &'static str,
        reason: Option<String>,
    },
    /// 什么都不做（重复触发、陈旧回调、终态被再次推进）
    Ignore { reason: String },
}

/// 人工闸门边界。四个变体都是「进入下一个角色」语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateBoundary {
    DevStart,
    ReviewStart,
    DevRestart,
    TestStart,
}

impl GateBoundary {
    /// 配置 gates 数组里的取值：dev_start / review_start / dev_restart / test_start
    pub fn as_str(self) -> &'static str {
        match self {
            GateBoundary::DevStart => "dev_start",
            GateBoundary::ReviewStart => "review_start",
            GateBoundary::DevRestart => "dev_restart",
            GateBoundary::TestStart => "test_start",
        }
    }

    /// 该边界对应的等待状态（pending_*_gate / pending_confirm）
    pub fn pending_status(self) -> &'static str {
        match self {
            GateBoundary::DevStart => STATUS_PENDING_CONFIRM,
            GateBoundary::ReviewStart => STATUS_PENDING_REVIEW_GATE,
            GateBoundary::DevRestart => STATUS_PENDING_DEV_GATE,
            GateBoundary::TestStart => STATUS_PENDING_TEST_GATE,
        }
    }
}

/// 闸门应答是肯定还是否定。比较前 trim()。
/// 肯定：开始修 / 继续 / 继续评审 / 重新开发 / 继续测试 / 确认 / 是
/// 否定：跳过 / 终止 / 取消 / 否
/// 其余返回 None（调用方转成 Ignore，不猜——猜错的代价是在错误的 thread 上派发）
pub fn gate_answer_is_yes(answer: &str) -> Option<bool> {
    let a = answer.trim();
    match a {
        "开始修" | "继续" | "继续评审" | "重新开发" | "继续测试" | "确认" | "是" => {
            Some(true)
        }
        "跳过" | "终止" | "取消" | "否" => Some(false),
        _ => None,
    }
}

/// 进入 `next_role` 时对应的闸门边界（若有）。
/// 开发角色入口是 DevStart / DevRestart，不走这条（由 pending_confirm 与 Reject 路径处理）。
fn gate_boundary_for_entering(next_role: &str) -> Option<GateBoundary> {
    match next_role {
        "reviewer" => Some(GateBoundary::ReviewStart),
        "tester" => Some(GateBoundary::TestStart),
        _ => None,
    }
}

fn role_label(role: &str) -> &str {
    role_def(role).map(|d| d.label).unwrap_or(role)
}

fn trigger_kind(t: &Trigger<'_>) -> &'static str {
    match t {
        Trigger::GateAnswered { .. } => "GateAnswered",
        Trigger::RunFinished { .. } => "RunFinished",
        Trigger::Cancelled { .. } => "Cancelled",
    }
}

fn cfg_has_gate(cfg: &TeamTaskSettings, boundary: GateBoundary) -> bool {
    cfg.gates.iter().any(|g| g == boundary.as_str())
}

/// 状态机唯一判定点。纯函数：同样的输入永远给同样的输出，可单测全分支。
pub fn decide_next(input: &DecideInput<'_>, cfg: &TeamTaskSettings) -> Decision {
    // --- 规则 A：终态与取消 ---
    if is_terminal(input.status) {
        return Decision::Ignore {
            reason: format!("任务已是终态 {}", input.status),
        };
    }

    if let Trigger::Cancelled { operator } = &input.trigger {
        return Decision::Finish {
            status: STATUS_CANCELLED,
            reason: Some(format!("由 {operator} 取消")),
        };
    }

    // --- 规则 B：待确认（pending_confirm，即现有 task_gate）---
    if input.status == STATUS_PENDING_CONFIRM {
        return match &input.trigger {
            Trigger::GateAnswered { answer } => match gate_answer_is_yes(answer) {
                Some(true) => match first_role(&cfg.roles) {
                    Some(role) => Decision::DispatchRole { role, round: 1 },
                    None => Decision::Finish {
                        status: STATUS_FAILED,
                        reason: Some("未配置任何角色".to_string()),
                    },
                },
                Some(false) => Decision::Finish {
                    status: STATUS_CANCELLED,
                    reason: Some("用户跳过".to_string()),
                },
                None => Decision::Ignore {
                    reason: format!("无法识别的闸门应答: {}", answer.trim()),
                },
            },
            other => Decision::Ignore {
                reason: format!(
                    "status={} 不接受 trigger={}",
                    input.status,
                    trigger_kind(other)
                ),
            },
        };
    }

    // --- 规则 C：角色运行中收到 RunFinished ---
    if is_running(input.status) {
        return match &input.trigger {
            Trigger::RunFinished {
                role,
                round: _,
                outcome,
            } => {
                let current = input.current_role.unwrap_or("");
                if *role != current {
                    return Decision::Ignore {
                        reason: format!("陈旧回调: current_role={current}, finished_role={role}"),
                    };
                }
                let label = role_label(role);
                match outcome {
                    RunOutcome::Failed => Decision::Finish {
                        status: STATUS_FAILED,
                        reason: Some(format!("{label} 角色执行失败")),
                    },
                    // Verdict::Unknown 一律 failed，不开人工闸门。理由：
                    // (a) gates=[] 语义是全自动无人值守，飞书交互单不过期，
                    //     开闸门会让任务永远挂着等不会有人点的按钮，僵尸态比 failed 更糟；
                    // (b) 最后一个角色（tester）返回 Unknown 时没有边界可开——
                    //     GateBoundary 四个变体全是「进入下一个角色」语义；
                    // (c) Unknown 是异常路径，加人工兜底会掩盖 prompt 的问题。
                    RunOutcome::Finished(Verdict::Unknown) => Decision::Finish {
                        status: STATUS_FAILED,
                        reason: Some(format!("{label} 结论无法解析")),
                    },
                    RunOutcome::Finished(Verdict::Pass) => match next_role(&cfg.roles, role) {
                        Some(next) => {
                            if let Some(boundary) = gate_boundary_for_entering(&next) {
                                if cfg_has_gate(cfg, boundary) {
                                    return Decision::OpenGate { boundary };
                                }
                            }
                            Decision::DispatchRole {
                                role: next,
                                round: 1,
                            }
                        }
                        None => Decision::Finish {
                            status: STATUS_DONE,
                            reason: None,
                        },
                    },
                    RunOutcome::Finished(Verdict::Reject) => {
                        let needs_verdict = role_def(role).map(|d| d.needs_verdict).unwrap_or(true);
                        // 开发不该有 reject 语义，静默当打回会掩盖真 bug
                        if !needs_verdict {
                            return Decision::Finish {
                                status: STATUS_FAILED,
                                reason: Some("开发角色返回 reject，prompt 或解析异常".to_string()),
                            };
                        }
                        if input.dev_rounds >= cfg.max_dev_rounds {
                            return Decision::Finish {
                                status: STATUS_FAILED,
                                reason: Some(format!(
                                    "已达最大返工轮次 {}，请人工接手",
                                    cfg.max_dev_rounds
                                )),
                            };
                        }
                        if cfg_has_gate(cfg, GateBoundary::DevRestart) {
                            Decision::OpenGate {
                                boundary: GateBoundary::DevRestart,
                            }
                        } else {
                            Decision::DispatchRole {
                                role: "developer".to_string(),
                                round: input.dev_rounds + 1,
                            }
                        }
                    }
                }
            }
            other => Decision::Ignore {
                reason: format!(
                    "status={} 不接受 trigger={}",
                    input.status,
                    trigger_kind(other)
                ),
            },
        };
    }

    // --- 规则 D：等待闸门（pending_*_gate）收到 GateAnswered ---
    if matches!(
        input.status,
        STATUS_PENDING_REVIEW_GATE | STATUS_PENDING_DEV_GATE | STATUS_PENDING_TEST_GATE
    ) {
        return match &input.trigger {
            Trigger::GateAnswered { answer } => match gate_answer_is_yes(answer) {
                Some(true) => dispatch_from_pending_gate(input, cfg),
                Some(false) => {
                    let boundary = pending_status_to_boundary(input.status)
                        .map(|b| b.as_str())
                        .unwrap_or(input.status);
                    Decision::Finish {
                        status: STATUS_CANCELLED,
                        reason: Some(format!("用户在 {boundary} 终止")),
                    }
                }
                None => Decision::Ignore {
                    reason: format!("无法识别的闸门应答: {}", answer.trim()),
                },
            },
            other => Decision::Ignore {
                reason: format!(
                    "status={} 不接受 trigger={}",
                    input.status,
                    trigger_kind(other)
                ),
            },
        };
    }

    // --- 规则 E：兜底 ---
    Decision::Ignore {
        reason: format!(
            "status={} 不接受 trigger={}",
            input.status,
            trigger_kind(&input.trigger)
        ),
    }
}

fn pending_status_to_boundary(status: &str) -> Option<GateBoundary> {
    match status {
        STATUS_PENDING_CONFIRM => Some(GateBoundary::DevStart),
        STATUS_PENDING_REVIEW_GATE => Some(GateBoundary::ReviewStart),
        STATUS_PENDING_DEV_GATE => Some(GateBoundary::DevRestart),
        STATUS_PENDING_TEST_GATE => Some(GateBoundary::TestStart),
        _ => None,
    }
}

fn dispatch_from_pending_gate(input: &DecideInput<'_>, cfg: &TeamTaskSettings) -> Decision {
    match input.status {
        STATUS_PENDING_REVIEW_GATE => {
            let current = input.current_role.unwrap_or("developer");
            match next_role(&cfg.roles, current) {
                Some(role) => Decision::DispatchRole { role, round: 1 },
                None => Decision::Finish {
                    status: STATUS_FAILED,
                    reason: Some("没有下一个角色可进入评审".to_string()),
                },
            }
        }
        STATUS_PENDING_DEV_GATE => Decision::DispatchRole {
            role: "developer".to_string(),
            round: input.dev_rounds + 1,
        },
        STATUS_PENDING_TEST_GATE => {
            let current = input.current_role.unwrap_or("reviewer");
            match next_role(&cfg.roles, current) {
                Some(role) => Decision::DispatchRole { role, round: 1 },
                // 与 pending_review_gate 兜底口径一致：不要硬编码派发可能不在
                // cfg.roles 里的角色（配置被裁剪时会静默走错）。
                None => Decision::Finish {
                    status: STATUS_FAILED,
                    reason: Some("没有下一个角色可进入测试".to_string()),
                },
            }
        }
        other => Decision::Ignore {
            reason: format!("未知的 pending 状态: {other}"),
        },
    }
}

// ---------------------------------------------------------------------------
// parse_handoff
// ---------------------------------------------------------------------------

/// 角色输出末尾的结构化交接产物。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Handoff {
    pub verdict: Option<Verdict>,
    pub changed_files: Option<i32>,
    pub summary: Option<String>,
    /// 阻塞项；原文写 none / 无 / 空 一律归一成 None
    pub blocking: Option<String>,
}

/// 从 run 的最终文本里宽松提取「## 交接」段。
///
/// 宽松是刻意的——模型常加前言、改标题层级（##/###）、漏字段、用全角冒号。
/// 解析失败**不算 run 失败**：全文找不到任何可识别字段时返回 Handoff::default()，
/// 由调用方按角色兜底（开发角色用 git diff 补 changed_files，
/// 评审/测试角色的 verdict 为 None 时视为 Verdict::Unknown → 任务 failed）。
pub fn parse_handoff(text: &str) -> Handoff {
    let section = extract_handoff_section(text);
    let body = section.as_deref().unwrap_or(text);
    parse_handoff_fields(body)
}

/// 定位「## 交接」/「### 交接」标题后的正文；找不到返回 None。
///
/// 取**最后一个**「交接」标题，不是第一个。prompt 要求模型「在回复的最后输出」，
/// 而模型常把 prompt 里的格式说明一起抄回来，形成两个 `## 交接`——
/// 取第一个会拿到说明文字（没有 key: value），三个字段全空，
/// 评审 verdict 变 Unknown 导致任务莫名 failed。正文里顺口提到「交接」
/// 的散段同理会被前面的匹配挡掉。
fn extract_handoff_section(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut start = None;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if !trimmed.contains("交接") {
            continue;
        }
        // 兼容 "## 交接" / "###交接" 等：2～3 个 # 开头即可
        let hash_count = trimmed.chars().take_while(|c| *c == '#').count();
        if (2..=3).contains(&hash_count) {
            // 不 break：继续扫完，start 一路覆盖到最后一个匹配
            start = Some(i + 1);
        }
    }
    let start = start?;
    // 取到下一个 markdown 标题，或文末
    let mut end = lines.len();
    for (i, line) in lines.iter().enumerate().skip(start) {
        let trimmed = line.trim();
        let hash_count = trimmed.chars().take_while(|c| *c == '#').count();
        if !(1..=6).contains(&hash_count) || trimmed.len() <= hash_count {
            continue;
        }
        let rest = &trimmed[hash_count..];
        if rest.starts_with(' ') || rest.starts_with('\u{3000}') {
            end = i;
            break;
        }
    }
    Some(lines[start..end].join("\n"))
}

fn parse_handoff_fields(body: &str) -> Handoff {
    let mut handoff = Handoff::default();
    let mut seen_verdict = false;
    let mut seen_changed = false;
    let mut seen_summary = false;
    let mut seen_blocking = false;

    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (key, value) = match split_kv(trimmed) {
            Some(kv) => kv,
            None => continue,
        };
        let key_l = key.to_ascii_lowercase();
        match key_l.as_str() {
            "verdict" if !seen_verdict => {
                seen_verdict = true;
                let v = Verdict::parse(value);
                // 解析成 Unknown 仍记为 Some(Unknown)，让调用方区分「没写」与「写了但看不懂」
                handoff.verdict = Some(v);
            }
            "changed_files" if !seen_changed => {
                seen_changed = true;
                let v = value.trim();
                handoff.changed_files = v.parse::<i32>().ok();
            }
            "summary" if !seen_summary => {
                seen_summary = true;
                let s = truncate_chars(value.trim(), 500);
                if !s.is_empty() {
                    handoff.summary = Some(s);
                }
            }
            "blocking" if !seen_blocking => {
                seen_blocking = true;
                let v = value.trim();
                let lower = v.to_ascii_lowercase();
                if v.is_empty() || lower == "none" || v == "无" {
                    handoff.blocking = None;
                } else {
                    handoff.blocking = Some(v.to_string());
                }
            }
            _ => {}
        }
    }
    handoff
}

/// 按半角 `:` 或全角 `：` 拆 key/value；找不到分隔符返回 None。
fn split_kv(line: &str) -> Option<(&str, &str)> {
    if let Some(idx) = line.find(':') {
        // 避免把全角 `：` 的 UTF-8 误用 byte index；先查全角
        if let Some(idx_full) = line.find('：') {
            if idx_full < idx {
                let (k, v) = line.split_at(idx_full);
                return Some((k.trim(), v.trim_start_matches('：').trim()));
            }
        }
        let (k, v) = line.split_at(idx);
        return Some((k.trim(), v.trim_start_matches(':').trim()));
    }
    if let Some(idx) = line.find('：') {
        let (k, v) = line.split_at(idx);
        return Some((k.trim(), v.trim_start_matches('：').trim()));
    }
    None
}

/// 按 Unicode 字符截断，不按字节，避免切坏 UTF-8 / 中文。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

// ---------------------------------------------------------------------------
// 单测
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 单测构造辅助：只改 roles/gates/max，其余用合法默认。
    fn test_settings(
        roles: Vec<String>,
        gates: Vec<String>,
        max_dev_rounds: i32,
    ) -> TeamTaskSettings {
        TeamTaskSettings {
            task_gate_enabled: true,
            enabled: true,
            roles,
            gates,
            max_dev_rounds,
            dashboard_base_url: None,
            updated_by: None,
        }
    }

    fn auto_cfg() -> TeamTaskSettings {
        test_settings(default_roles(), vec![], 3)
    }

    fn all_gates_cfg() -> TeamTaskSettings {
        test_settings(
            default_roles(),
            vec![
                "dev_start".into(),
                "review_start".into(),
                "dev_restart".into(),
                "test_start".into(),
            ],
            3,
        )
    }

    fn default_roles() -> Vec<String> {
        vec!["developer".into(), "reviewer".into(), "tester".into()]
    }

    fn input_pending_yes() -> DecideInput<'static> {
        DecideInput {
            status: STATUS_PENDING_CONFIRM,
            current_role: None,
            dev_rounds: 0,
            trigger: Trigger::GateAnswered {
                answer: "开始修"
            },
        }
    }

    // ----- role helpers -----

    #[test]
    fn role_def_known_and_unknown() {
        let dev = role_def("developer").unwrap();
        assert_eq!(dev.label, "开发");
        assert_eq!(dev.running_status, "running_developer");
        assert!(!dev.needs_verdict);
        assert!(role_def("reviewer").unwrap().needs_verdict);
        assert_eq!(
            role_def("reviewer").unwrap().running_status,
            "running_reviewer"
        );
        assert_eq!(role_def("tester").unwrap().running_status, "running_tester");
        assert!(role_def("docs").is_none());
        assert!(role_def("").is_none());
    }

    #[test]
    fn next_role_and_first_role() {
        let roles = default_roles();
        assert_eq!(first_role(&roles).as_deref(), Some("developer"));
        assert_eq!(next_role(&roles, "developer").as_deref(), Some("reviewer"));
        assert_eq!(next_role(&roles, "reviewer").as_deref(), Some("tester"));
        assert_eq!(next_role(&roles, "tester"), None);
        assert_eq!(next_role(&roles, "unknown"), None);

        let empty: Vec<String> = vec![];
        assert_eq!(first_role(&empty), None);
        assert_eq!(next_role(&empty, "developer"), None);

        let solo = vec!["developer".to_string()];
        assert_eq!(next_role(&solo, "developer"), None);
    }

    #[test]
    fn is_terminal_and_is_running() {
        assert!(is_terminal(STATUS_DONE));
        assert!(is_terminal(STATUS_FAILED));
        assert!(is_terminal(STATUS_CANCELLED));
        assert!(!is_terminal(STATUS_PENDING_CONFIRM));
        assert!(!is_terminal("running_developer"));

        assert!(is_running("running_developer"));
        assert!(is_running("running_reviewer"));
        assert!(is_running("running_tester"));
        assert!(!is_running(STATUS_PENDING_CONFIRM));
        assert!(!is_running(STATUS_DONE));
    }

    // ----- Verdict -----

    #[test]
    fn verdict_parse_loose() {
        assert_eq!(Verdict::parse("pass"), Verdict::Pass);
        assert_eq!(Verdict::parse(" PASS "), Verdict::Pass);
        assert_eq!(Verdict::parse("pass。"), Verdict::Pass);
        assert_eq!(Verdict::parse("通过"), Verdict::Pass);
        assert_eq!(Verdict::parse("approved"), Verdict::Pass);
        assert_eq!(Verdict::parse("reject"), Verdict::Reject);
        assert_eq!(Verdict::parse("打回"), Verdict::Reject);
        assert_eq!(Verdict::parse("不通过"), Verdict::Reject);
        assert_eq!(Verdict::parse("rejected,"), Verdict::Reject);
        assert_eq!(Verdict::parse("maybe"), Verdict::Unknown);
        assert_eq!(Verdict::parse(""), Verdict::Unknown);
        assert_eq!(Verdict::Pass.as_str(), "pass");
        assert_eq!(Verdict::Reject.as_str(), "reject");
        assert_eq!(Verdict::Unknown.as_str(), "unknown");
    }

    // ----- gate_answer_is_yes -----

    #[test]
    fn gate_answer_yes_no_unknown() {
        for a in [
            "开始修",
            "继续",
            "继续评审",
            "重新开发",
            "继续测试",
            "确认",
            "是",
        ] {
            assert_eq!(gate_answer_is_yes(a), Some(true), "yes: {a}");
        }
        for a in ["跳过", "终止", "取消", "否"] {
            assert_eq!(gate_answer_is_yes(a), Some(false), "no: {a}");
        }
        assert_eq!(gate_answer_is_yes("  继续  "), Some(true));
        assert_eq!(gate_answer_is_yes("随便"), None);
        assert_eq!(gate_answer_is_yes(""), None);
    }

    // ----- decide_next: 全自动流水线 -----

    #[test]
    fn auto_full_pipeline_pass() {
        let cfg = auto_cfg();

        // pending_confirm + 肯定 → developer
        let d = decide_next(&input_pending_yes(), &cfg);
        assert_eq!(
            d,
            Decision::DispatchRole {
                role: "developer".into(),
                round: 1
            }
        );

        // developer Pass → reviewer 直接派发
        let d = decide_next(
            &DecideInput {
                status: "running_developer",
                current_role: Some("developer"),
                dev_rounds: 1,
                trigger: Trigger::RunFinished {
                    role: "developer",
                    round: 1,
                    outcome: RunOutcome::Finished(Verdict::Pass),
                },
            },
            &cfg,
        );
        assert_eq!(
            d,
            Decision::DispatchRole {
                role: "reviewer".into(),
                round: 1
            }
        );

        // reviewer Pass → tester
        let d = decide_next(
            &DecideInput {
                status: "running_reviewer",
                current_role: Some("reviewer"),
                dev_rounds: 1,
                trigger: Trigger::RunFinished {
                    role: "reviewer",
                    round: 1,
                    outcome: RunOutcome::Finished(Verdict::Pass),
                },
            },
            &cfg,
        );
        assert_eq!(
            d,
            Decision::DispatchRole {
                role: "tester".into(),
                round: 1
            }
        );

        // tester Pass → done
        let d = decide_next(
            &DecideInput {
                status: "running_tester",
                current_role: Some("tester"),
                dev_rounds: 1,
                trigger: Trigger::RunFinished {
                    role: "tester",
                    round: 1,
                    outcome: RunOutcome::Finished(Verdict::Pass),
                },
            },
            &cfg,
        );
        assert_eq!(
            d,
            Decision::Finish {
                status: STATUS_DONE,
                reason: None
            }
        );
    }

    // ----- decide_next: 四闸门 -----

    #[test]
    fn all_gates_open_at_each_boundary() {
        let cfg = all_gates_cfg();

        // developer Pass → OpenGate ReviewStart
        let d = decide_next(
            &DecideInput {
                status: "running_developer",
                current_role: Some("developer"),
                dev_rounds: 1,
                trigger: Trigger::RunFinished {
                    role: "developer",
                    round: 1,
                    outcome: RunOutcome::Finished(Verdict::Pass),
                },
            },
            &cfg,
        );
        assert_eq!(
            d,
            Decision::OpenGate {
                boundary: GateBoundary::ReviewStart
            }
        );

        // pending_review_gate + 继续 → reviewer
        let d = decide_next(
            &DecideInput {
                status: STATUS_PENDING_REVIEW_GATE,
                current_role: Some("developer"),
                dev_rounds: 1,
                trigger: Trigger::GateAnswered {
                    answer: "继续评审"
                },
            },
            &cfg,
        );
        assert_eq!(
            d,
            Decision::DispatchRole {
                role: "reviewer".into(),
                round: 1
            }
        );

        // reviewer Reject → OpenGate DevRestart
        let d = decide_next(
            &DecideInput {
                status: "running_reviewer",
                current_role: Some("reviewer"),
                dev_rounds: 1,
                trigger: Trigger::RunFinished {
                    role: "reviewer",
                    round: 1,
                    outcome: RunOutcome::Finished(Verdict::Reject),
                },
            },
            &cfg,
        );
        assert_eq!(
            d,
            Decision::OpenGate {
                boundary: GateBoundary::DevRestart
            }
        );

        // pending_dev_gate + 重新开发 → developer round 2
        let d = decide_next(
            &DecideInput {
                status: STATUS_PENDING_DEV_GATE,
                current_role: Some("reviewer"),
                dev_rounds: 1,
                trigger: Trigger::GateAnswered {
                    answer: "重新开发"
                },
            },
            &cfg,
        );
        assert_eq!(
            d,
            Decision::DispatchRole {
                role: "developer".into(),
                round: 2
            }
        );

        // reviewer Pass → OpenGate TestStart
        let d = decide_next(
            &DecideInput {
                status: "running_reviewer",
                current_role: Some("reviewer"),
                dev_rounds: 1,
                trigger: Trigger::RunFinished {
                    role: "reviewer",
                    round: 1,
                    outcome: RunOutcome::Finished(Verdict::Pass),
                },
            },
            &cfg,
        );
        assert_eq!(
            d,
            Decision::OpenGate {
                boundary: GateBoundary::TestStart
            }
        );

        // pending_test_gate + 继续测试 → tester
        let d = decide_next(
            &DecideInput {
                status: STATUS_PENDING_TEST_GATE,
                current_role: Some("reviewer"),
                dev_rounds: 1,
                trigger: Trigger::GateAnswered {
                    answer: "继续测试"
                },
            },
            &cfg,
        );
        assert_eq!(
            d,
            Decision::DispatchRole {
                role: "tester".into(),
                round: 1
            }
        );
    }

    // ----- decide_next: reject / max rounds -----

    #[test]
    fn reviewer_reject_redispatch_developer() {
        let cfg = auto_cfg();
        let d = decide_next(
            &DecideInput {
                status: "running_reviewer",
                current_role: Some("reviewer"),
                dev_rounds: 1,
                trigger: Trigger::RunFinished {
                    role: "reviewer",
                    round: 1,
                    outcome: RunOutcome::Finished(Verdict::Reject),
                },
            },
            &cfg,
        );
        assert_eq!(
            d,
            Decision::DispatchRole {
                role: "developer".into(),
                round: 2
            }
        );
    }

    #[test]
    fn reviewer_reject_at_max_rounds_fails() {
        let cfg = auto_cfg();
        let d = decide_next(
            &DecideInput {
                status: "running_reviewer",
                current_role: Some("reviewer"),
                dev_rounds: 3,
                trigger: Trigger::RunFinished {
                    role: "reviewer",
                    round: 1,
                    outcome: RunOutcome::Finished(Verdict::Reject),
                },
            },
            &cfg,
        );
        match d {
            Decision::Finish {
                status: STATUS_FAILED,
                reason: Some(r),
            } => {
                assert!(r.contains("最大返工轮次"), "reason={r}");
            }
            other => panic!("expected failed at max rounds, got {other:?}"),
        }
    }

    // ----- decide_next: Unknown → failed（三个角色，不开闸门）-----

    #[test]
    fn developer_unknown_fails() {
        let cfg = auto_cfg();
        let d = decide_next(
            &DecideInput {
                status: "running_developer",
                current_role: Some("developer"),
                dev_rounds: 1,
                trigger: Trigger::RunFinished {
                    role: "developer",
                    round: 1,
                    outcome: RunOutcome::Finished(Verdict::Unknown),
                },
            },
            &cfg,
        );
        match d {
            Decision::Finish {
                status: STATUS_FAILED,
                reason: Some(r),
            } => assert!(r.contains("无法解析"), "reason={r}"),
            other => panic!("expected failed, got {other:?}"),
        }
    }

    #[test]
    fn reviewer_unknown_fails() {
        let cfg = auto_cfg();
        let d = decide_next(
            &DecideInput {
                status: "running_reviewer",
                current_role: Some("reviewer"),
                dev_rounds: 1,
                trigger: Trigger::RunFinished {
                    role: "reviewer",
                    round: 1,
                    outcome: RunOutcome::Finished(Verdict::Unknown),
                },
            },
            &cfg,
        );
        match d {
            Decision::Finish {
                status: STATUS_FAILED,
                ..
            } => {}
            other => panic!("expected failed, got {other:?}"),
        }
    }

    #[test]
    fn tester_unknown_fails_not_open_gate() {
        // 回归：tester 之后没有「最终验收」边界，Unknown 不能 OpenGate
        let cfg = all_gates_cfg();
        let d = decide_next(
            &DecideInput {
                status: "running_tester",
                current_role: Some("tester"),
                dev_rounds: 1,
                trigger: Trigger::RunFinished {
                    role: "tester",
                    round: 1,
                    outcome: RunOutcome::Finished(Verdict::Unknown),
                },
            },
            &cfg,
        );
        match d {
            Decision::Finish {
                status: STATUS_FAILED,
                reason: Some(r),
            } => assert!(r.contains("无法解析"), "reason={r}"),
            Decision::OpenGate { .. } => panic!("Unknown 不应开闸门"),
            other => panic!("expected failed, got {other:?}"),
        }
    }

    #[test]
    fn developer_reject_fails() {
        let cfg = auto_cfg();
        let d = decide_next(
            &DecideInput {
                status: "running_developer",
                current_role: Some("developer"),
                dev_rounds: 1,
                trigger: Trigger::RunFinished {
                    role: "developer",
                    round: 1,
                    outcome: RunOutcome::Finished(Verdict::Reject),
                },
            },
            &cfg,
        );
        match d {
            Decision::Finish {
                status: STATUS_FAILED,
                reason: Some(r),
            } => assert!(r.contains("开发角色返回 reject"), "reason={r}"),
            other => panic!("expected failed, got {other:?}"),
        }
    }

    #[test]
    fn run_outcome_failed() {
        let cfg = auto_cfg();
        let d = decide_next(
            &DecideInput {
                status: "running_developer",
                current_role: Some("developer"),
                dev_rounds: 1,
                trigger: Trigger::RunFinished {
                    role: "developer",
                    round: 1,
                    outcome: RunOutcome::Failed,
                },
            },
            &cfg,
        );
        match d {
            Decision::Finish {
                status: STATUS_FAILED,
                reason: Some(r),
            } => assert!(r.contains("执行失败"), "reason={r}"),
            other => panic!("expected failed, got {other:?}"),
        }
    }

    #[test]
    fn stale_run_finished_ignored() {
        let cfg = auto_cfg();
        let d = decide_next(
            &DecideInput {
                status: "running_reviewer",
                current_role: Some("reviewer"),
                dev_rounds: 1,
                trigger: Trigger::RunFinished {
                    role: "developer",
                    round: 1,
                    outcome: RunOutcome::Finished(Verdict::Pass),
                },
            },
            &cfg,
        );
        match d {
            Decision::Ignore { reason } => {
                assert!(reason.contains("陈旧"), "reason={reason}");
                assert!(reason.contains("reviewer"));
                assert!(reason.contains("developer"));
            }
            other => panic!("expected Ignore, got {other:?}"),
        }
    }

    #[test]
    fn terminal_ignores_any_trigger() {
        let cfg = auto_cfg();
        for status in [STATUS_DONE, STATUS_FAILED, STATUS_CANCELLED] {
            let d = decide_next(
                &DecideInput {
                    status,
                    current_role: None,
                    dev_rounds: 1,
                    trigger: Trigger::GateAnswered {
                        answer: "开始修"
                    },
                },
                &cfg,
            );
            match d {
                Decision::Ignore { reason } => {
                    assert!(reason.contains("终态"), "status={status} reason={reason}");
                }
                other => panic!("expected Ignore for {status}, got {other:?}"),
            }
        }
    }

    #[test]
    fn cancelled_finishes() {
        let cfg = auto_cfg();
        let d = decide_next(
            &DecideInput {
                status: "running_developer",
                current_role: Some("developer"),
                dev_rounds: 1,
                trigger: Trigger::Cancelled {
                    operator: "admin:alice",
                },
            },
            &cfg,
        );
        assert_eq!(
            d,
            Decision::Finish {
                status: STATUS_CANCELLED,
                reason: Some("由 admin:alice 取消".into()),
            }
        );
    }

    #[test]
    fn single_role_developer_pass_done() {
        let cfg = test_settings(vec!["developer".into()], vec![], 3);
        let d = decide_next(
            &DecideInput {
                status: "running_developer",
                current_role: Some("developer"),
                dev_rounds: 1,
                trigger: Trigger::RunFinished {
                    role: "developer",
                    round: 1,
                    outcome: RunOutcome::Finished(Verdict::Pass),
                },
            },
            &cfg,
        );
        assert_eq!(
            d,
            Decision::Finish {
                status: STATUS_DONE,
                reason: None
            }
        );
    }

    #[test]
    fn empty_roles_pending_confirm_fails() {
        let cfg = test_settings(vec![], vec![], 3);
        let d = decide_next(&input_pending_yes(), &cfg);
        assert_eq!(
            d,
            Decision::Finish {
                status: STATUS_FAILED,
                reason: Some("未配置任何角色".into()),
            }
        );
    }

    #[test]
    fn pending_confirm_skip_and_unknown() {
        let cfg = auto_cfg();
        let d = decide_next(
            &DecideInput {
                status: STATUS_PENDING_CONFIRM,
                current_role: None,
                dev_rounds: 0,
                trigger: Trigger::GateAnswered { answer: "跳过" },
            },
            &cfg,
        );
        assert_eq!(
            d,
            Decision::Finish {
                status: STATUS_CANCELLED,
                reason: Some("用户跳过".into()),
            }
        );

        let d = decide_next(
            &DecideInput {
                status: STATUS_PENDING_CONFIRM,
                current_role: None,
                dev_rounds: 0,
                trigger: Trigger::GateAnswered {
                    answer: "随便点点"
                },
            },
            &cfg,
        );
        match d {
            Decision::Ignore { .. } => {}
            other => panic!("expected Ignore, got {other:?}"),
        }
    }

    #[test]
    fn pending_gate_no_cancels() {
        let cfg = all_gates_cfg();
        let d = decide_next(
            &DecideInput {
                status: STATUS_PENDING_REVIEW_GATE,
                current_role: Some("developer"),
                dev_rounds: 1,
                trigger: Trigger::GateAnswered { answer: "终止" },
            },
            &cfg,
        );
        match d {
            Decision::Finish {
                status: STATUS_CANCELLED,
                reason: Some(r),
            } => assert!(
                r.contains("review_start") || r.contains("终止"),
                "reason={r}"
            ),
            other => panic!("expected cancelled, got {other:?}"),
        }
    }

    /// 回归：pending_test_gate 找不到下一个角色时 Finish{failed}，
    /// 不要硬编码派发可能不在 cfg.roles 里的 "tester"。
    #[test]
    fn pending_test_gate_no_next_role_fails() {
        // 配置被裁剪成无 tester；current_role 仍是评审（开闸门时不清）
        let cfg = test_settings(
            vec!["developer".into(), "reviewer".into()],
            vec!["test_start".into()],
            3,
        );
        let d = decide_next(
            &DecideInput {
                status: STATUS_PENDING_TEST_GATE,
                current_role: Some("reviewer"),
                dev_rounds: 1,
                trigger: Trigger::GateAnswered {
                    answer: "继续测试"
                },
            },
            &cfg,
        );
        assert_eq!(
            d,
            Decision::Finish {
                status: STATUS_FAILED,
                reason: Some("没有下一个角色可进入测试".into()),
            }
        );
    }

    #[test]
    fn gate_boundary_helpers() {
        assert_eq!(GateBoundary::DevStart.as_str(), "dev_start");
        assert_eq!(
            GateBoundary::ReviewStart.pending_status(),
            STATUS_PENDING_REVIEW_GATE
        );
        assert_eq!(
            GateBoundary::DevRestart.pending_status(),
            STATUS_PENDING_DEV_GATE
        );
        assert_eq!(
            GateBoundary::TestStart.pending_status(),
            STATUS_PENDING_TEST_GATE
        );
    }

    // ----- parse_handoff -----

    #[test]
    fn parse_handoff_standard() {
        let text = r#"
做完了，改动如下。

## 交接
verdict: pass
changed_files: 3
summary: 修复登录超时
blocking: none
"#;
        let h = parse_handoff(text);
        assert_eq!(h.verdict, Some(Verdict::Pass));
        assert_eq!(h.changed_files, Some(3));
        assert_eq!(h.summary.as_deref(), Some("修复登录超时"));
        assert_eq!(h.blocking, None);
    }

    #[test]
    fn parse_handoff_fullwidth_colon() {
        let text = "### 交接\nverdict：pass\nchanged_files：2\nsummary：ok\nblocking：无\n";
        let h = parse_handoff(text);
        assert_eq!(h.verdict, Some(Verdict::Pass));
        assert_eq!(h.changed_files, Some(2));
        assert_eq!(h.summary.as_deref(), Some("ok"));
        assert_eq!(h.blocking, None);
    }

    #[test]
    fn parse_handoff_h3_and_preamble() {
        let text = r#"
我先分析了一下代码，然后做了修改。

以下是交接信息：

### 交接产物
verdict: reject
changed_files: 1
summary: 缺测试
blocking: 需要补单元测试
"#;
        let h = parse_handoff(text);
        assert_eq!(h.verdict, Some(Verdict::Reject));
        assert_eq!(h.changed_files, Some(1));
        assert_eq!(h.blocking.as_deref(), Some("需要补单元测试"));
    }

    #[test]
    fn parse_handoff_non_numeric_changed_files() {
        let text = "## 交接\nchanged_files: 很多\nsummary: x\n";
        let h = parse_handoff(text);
        assert_eq!(h.changed_files, None);
        assert_eq!(h.summary.as_deref(), Some("x"));
    }

    #[test]
    fn parse_handoff_blocking_none_variants() {
        let h = parse_handoff("## 交接\nblocking: none\n");
        assert_eq!(h.blocking, None);
        let h = parse_handoff("## 交接\nblocking: 无\n");
        assert_eq!(h.blocking, None);
        let h = parse_handoff("## 交接\nblocking:   \n");
        assert_eq!(h.blocking, None);
    }

    #[test]
    fn parse_handoff_no_section_defaults() {
        let h = parse_handoff("只是一段普通输出，没有交接。");
        assert_eq!(h, Handoff::default());
    }

    #[test]
    fn parse_handoff_summary_char_truncate() {
        // 600 个中文字符，截到 500
        let long: String = "中".repeat(600);
        let text = format!("## 交接\nsummary: {long}\n");
        let h = parse_handoff(&text);
        let s = h.summary.expect("summary");
        assert_eq!(s.chars().count(), 500);
        assert!(s.chars().all(|c| c == '中'));
    }

    #[test]
    fn parse_handoff_first_key_wins() {
        let text = "## 交接\nverdict: pass\nverdict: reject\n";
        let h = parse_handoff(text);
        assert_eq!(h.verdict, Some(Verdict::Pass));
    }

    #[test]
    fn parse_handoff_fallback_full_text_scan() {
        // 没有交接标题，仍能从全文扫到 key: value
        let text = "blah\nverdict: pass\nchanged_files: 5\n";
        let h = parse_handoff(text);
        assert_eq!(h.verdict, Some(Verdict::Pass));
        assert_eq!(h.changed_files, Some(5));
    }

    #[test]
    fn parse_handoff_takes_last_section_on_echoed_instruction() {
        // 模型把 prompt 里的格式说明一起抄回，形成两个 ## 交接。
        // 取第一个会拿到说明文字（无 key:value），三个字段全空 → 任务 failed。
        // 必须取最后一个，才能解析到真实结论。
        let text = "\
好的，我按格式输出。

## 交接
在回复的**最后**输出下面这段，键名与格式不要改动：

## 交接
verdict: pass
changed_files: 4
summary: 真实结论
blocking: none
";
        let h = parse_handoff(text);
        assert_eq!(h.verdict, Some(Verdict::Pass));
        assert_eq!(h.changed_files, Some(4));
        assert_eq!(h.summary.as_deref(), Some("真实结论"));
        assert_eq!(h.blocking, None);
    }
}
