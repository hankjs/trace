//! 团队任务飞书主卡：构造 + 生命周期同步。
//!
//! 一张卡片贯穿整条流水线原地刷新（`card_message_id` 存在 `team_tasks`）。
//! 首次 reply 到闸门卡的 `origin_message_id` 生成，之后 update_card 刷新。

use super::role_def;
use crate::feishu::card::build_progress_bar;
use crate::AppState;
use hank_db::{TeamTask, TeamTaskRun};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// 主卡入参
// ---------------------------------------------------------------------------

/// 团队任务主卡入参。一张卡片贯穿整条流水线原地刷新，
/// 所以它要能表达「任意角色 / 任意阶段」的状态。
pub struct TeamStageCardOptions {
    pub task_no: String,
    pub goal: String,
    pub status: String,
    /// 当前角色 id；终态为 None
    pub current_role: Option<String>,
    pub issue_key: Option<String>,
    pub source_label: String,
    pub backend: String,
    pub dev_rounds: i32,
    /// 已完成 + 进行中的轮次，用于渲染「流转记录」
    pub runs: Vec<TeamStageRun>,
    /// 当前进度（来自 TaskRegistry 快照）；无则不渲染进度区
    pub progress: Option<TeamStageProgress>,
    pub dashboard_url: Option<String>,
    /// 终态说明（失败原因 / 取消理由）
    pub reason: Option<String>,
}

pub struct TeamStageRun {
    pub role_label: String,
    pub round: i32,
    pub status: String,
    pub verdict: Option<String>,
    pub summary: Option<String>,
    pub dirty_files: Option<i32>,
}

pub struct TeamStageProgress {
    pub percent: u32,
    pub detail: String,
    pub activities: Vec<String>,
}

// ---------------------------------------------------------------------------
// 标题 / 配色
// ---------------------------------------------------------------------------

/// 标题随状态变化，对齐设计文档 §7.1 与任务 A1.2。
fn card_title(status: &str, current_role: Option<&str>) -> String {
    match status {
        "running_developer" => "团队任务 · 开发 · 进行中".into(),
        "running_reviewer" => "团队任务 · 评审 · 进行中".into(),
        "running_tester" => "团队任务 · 测试 · 进行中".into(),
        "pending_confirm" => "团队任务 · 待确认".into(),
        "pending_review_gate" => "团队任务 · 开发完成 · 待进入评审".into(),
        "pending_dev_gate" => "团队任务 · 评审 → 开发（打回）".into(),
        "pending_test_gate" => "团队任务 · 评审通过 · 待进入测试".into(),
        "done" => "团队任务 · 已完成".into(),
        "failed" => "团队任务 · 失败".into(),
        "cancelled" => "团队任务 · 已取消".into(),
        other if other.starts_with("running_") => {
            let label = current_role
                .and_then(role_def)
                .map(|d| d.label)
                .unwrap_or("角色");
            format!("团队任务 · {label} · 进行中")
        }
        other if other.starts_with("pending_") => "团队任务 · 待放行".into(),
        _ => "团队任务".into(),
    }
}

fn card_template(status: &str) -> &'static str {
    if status.starts_with("running_") {
        "blue"
    } else if status.starts_with("pending_") {
        "orange"
    } else {
        match status {
            "done" => "green",
            "failed" => "red",
            "cancelled" => "grey",
            _ => "blue",
        }
    }
}

fn status_display(status: &str) -> &str {
    match status {
        "pending_confirm" => "待确认",
        "running_developer" => "开发中",
        "pending_review_gate" => "待进入评审",
        "running_reviewer" => "评审中",
        "pending_dev_gate" => "待重新开发",
        "pending_test_gate" => "待进入测试",
        "running_tester" => "测试中",
        "done" => "已完成",
        "failed" => "失败",
        "cancelled" => "已取消",
        other => other,
    }
}

fn role_label_of(role: Option<&str>) -> String {
    role.and_then(role_def)
        .map(|d| d.label.to_string())
        .unwrap_or_else(|| "—".into())
}

/// 按 Unicode 字符截断，不按字节，避免切坏中文。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

fn format_run_line(run: &TeamStageRun) -> String {
    let summary_short = run
        .summary
        .as_deref()
        .map(|s| truncate_chars(s, 80))
        .filter(|s| !s.is_empty());

    if run.status == "running" {
        return format!("🔄 {} 第{}轮 · 进行中", run.role_label, run.round);
    }

    let is_reject = run.verdict.as_deref() == Some("reject");
    let is_failed = run.status == "failed" || run.verdict.as_deref() == Some("failed");

    if is_reject {
        let reason = summary_short.unwrap_or_else(|| "打回".into());
        return format!("❌ {} 第{}轮 · 打回：{}", run.role_label, run.round, reason);
    }
    if is_failed {
        let reason = summary_short.unwrap_or_else(|| "失败".into());
        return format!("❌ {} 第{}轮 · {}", run.role_label, run.round, reason);
    }

    // finished / pass
    let mut line = format!("✅ {} 第{}轮", run.role_label, run.round);
    if let Some(n) = run.dirty_files {
        line.push_str(&format!(" · 改动 {n} 个文件"));
    } else if let Some(s) = summary_short {
        line.push_str(&format!(" · {s}"));
    }
    line
}

// ---------------------------------------------------------------------------
// 构造
// ---------------------------------------------------------------------------

/// 团队任务主卡（schema 2.0，update_multi）。
///
/// 标题随状态变化，对齐设计文档 §7.1：
///   running_developer     → 团队任务 · 开发 · 进行中
///   pending_review_gate   → 团队任务 · 开发完成 · 待进入评审
///   pending_dev_gate      → 团队任务 · 评审 → 开发（打回）
///   done / failed / cancelled → 团队任务 · 已完成 / 失败 / 已取消
pub fn build_team_stage_card(opts: &TeamStageCardOptions) -> Value {
    let title = card_title(&opts.status, opts.current_role.as_deref());
    let template = card_template(&opts.status);
    let goal = truncate_chars(&opts.goal, 500);

    // 1. 目标
    let mut elements: Vec<Value> = vec![json!({
        "tag": "markdown",
        "content": format!("**目标**\n{goal}")
    })];

    // 2. 基本信息；issue_key 为 None 时不渲染该格
    let mut info_lines = vec![
        format!(
            "任务编号 `{}`   状态 {}",
            opts.task_no,
            status_display(&opts.status)
        ),
        format!(
            "当前角色 {}   来源 {}\n后端 {}   开发轮次 {}",
            role_label_of(opts.current_role.as_deref()),
            opts.source_label,
            opts.backend,
            opts.dev_rounds
        ),
    ];
    if let Some(key) = opts.issue_key.as_deref().filter(|s| !s.is_empty()) {
        info_lines.insert(1, format!("Issue `{key}`"));
    }
    elements.push(json!({
        "tag": "markdown",
        "content": format!("**基本信息**\n{}", info_lines.join("\n"))
    }));

    // 3. hr
    elements.push(json!({ "tag": "hr" }));

    // 4. 流转记录
    let run_lines: Vec<String> = opts.runs.iter().map(format_run_line).collect();
    let runs_body = if run_lines.is_empty() {
        "（尚无轮次）".to_string()
    } else {
        run_lines.join("\n")
    };
    elements.push(json!({
        "tag": "markdown",
        "content": format!("**流转记录**\n{runs_body}")
    }));

    // 5. 当前进展（有 progress 时）
    if let Some(p) = &opts.progress {
        let percent = p.percent.min(100);
        let activity_text = if p.activities.is_empty() {
            String::new()
        } else {
            let items = p
                .activities
                .iter()
                .rev()
                .take(5)
                .map(|a| format!("- {a}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!("\n\n**最近活动**\n{items}")
        };
        elements.push(json!({
            "tag": "markdown",
            "content": format!(
                "**当前进展**\n{} {}%\n\n**当前：** {}{}",
                build_progress_bar(percent),
                percent,
                p.detail,
                activity_text
            )
        }));
    }

    // 6. 终态说明
    if let Some(reason) = opts.reason.as_deref().filter(|s| !s.is_empty()) {
        elements.push(json!({
            "tag": "markdown",
            "content": format!("**说明**\n{reason}")
        }));
    }

    // 7. 看板链接；None 时整行不渲染
    if let Some(url) = opts.dashboard_url.as_deref().filter(|s| !s.is_empty()) {
        elements.push(json!({
            "tag": "markdown",
            "content": format!("[在看板查看]({url})")
        }));
    }

    json!({
        "schema": "2.0",
        "config": {
            "update_multi": true,
            "summary": { "content": title }
        },
        "header": {
            "template": template,
            "title": { "tag": "plain_text", "content": title }
        },
        "body": {
            "direction": "vertical",
            "elements": elements
        }
    })
}

// ---------------------------------------------------------------------------
// 看板深链（格式只在这一处定义，仿 admin_interaction_url）
// ---------------------------------------------------------------------------

/// 看板任务详情深链。`base` 为 `dashboard_base_url`；
/// 格式 `{base}/#/team/{task_no}`，只在本函数定义，避免与前端 hash 路由漂移。
///
/// 用 `#/team/`（带斜杠）对齐 Vue `createWebHashHistory` 的默认形态；
/// 截图里的 `#team/` 是同一深链的简写，打开时路由按 `#/team/` 解析。
pub(crate) fn build_dashboard_task_url(base: &str, task_no: &str) -> String {
    let base = base.trim_end_matches('/');
    format!("{base}/#/team/{task_no}")
}

// ---------------------------------------------------------------------------
// 节流
// ---------------------------------------------------------------------------

/// 主卡刷新最小间隔。
///
/// 不用 `ThrottledCardUpdater`：它面向「单个角色 run 期间的连续 push」
/// （持有 UpdateFn 闭包 + finish/cancel 生命周期），而主卡跨角色、
/// 在离散状态点 best-effort 同步，形状不适配。这里用进程内
/// `task_id → Instant` 做最小间隔，防止 finalize+dispatch 连刷撞频控。
const SYNC_MIN_INTERVAL: Duration = Duration::from_secs(2);

fn last_sync_map() -> &'static Mutex<HashMap<String, Instant>> {
    static MAP: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn should_throttle(task_id: &str) -> bool {
    let mut map = match last_sync_map().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let now = Instant::now();
    if let Some(prev) = map.get(task_id) {
        if now.duration_since(*prev) < SYNC_MIN_INTERVAL {
            return true;
        }
    }
    map.insert(task_id.to_string(), now);
    false
}

/// 强制刷新时清掉节流记录（终态 / 首次建卡需要立刻可见）。
fn clear_throttle(task_id: &str) {
    if let Ok(mut map) = last_sync_map().lock() {
        map.remove(task_id);
    }
}

// ---------------------------------------------------------------------------
// 生命周期
// ---------------------------------------------------------------------------

fn source_label(source: &str) -> String {
    match source {
        "feishu" => "飞书派单".into(),
        "dashboard" => "看板".into(),
        "" => "飞书派单".into(),
        other => other.to_string(),
    }
}

fn runs_to_stage(runs: &[TeamTaskRun]) -> Vec<TeamStageRun> {
    runs.iter()
        .map(|r| TeamStageRun {
            role_label: role_def(&r.role)
                .map(|d| d.label.to_string())
                .unwrap_or_else(|| r.role.clone()),
            round: r.round,
            status: r.status.clone(),
            verdict: r.verdict.clone(),
            summary: r.summary.clone(),
            dirty_files: r.dirty_files,
        })
        .collect()
}

async fn assemble_options(state: &Arc<AppState>, task: &TeamTask) -> TeamStageCardOptions {
    let runs = state.db.list_team_runs(&task.id).await.unwrap_or_default();
    let progress = state
        .tasks
        .progress(&task.session_id)
        .await
        .map(|p| TeamStageProgress {
            percent: p.percent,
            detail: p.detail,
            activities: p.activities,
        });
    let settings = super::settings::effective(state).await;
    let dashboard_url = settings
        .dashboard_base_url
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|base| build_dashboard_task_url(base, &task.task_no));

    let reason = if super::is_terminal(&task.status) {
        task.error.clone()
    } else {
        None
    };

    TeamStageCardOptions {
        task_no: task.task_no.clone(),
        goal: task
            .goal
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| task.title.clone()),
        status: task.status.clone(),
        current_role: task.current_role.clone(),
        issue_key: task.issue_key.clone(),
        source_label: source_label(&task.source),
        backend: task.backend.clone(),
        dev_rounds: task.dev_rounds,
        runs: runs_to_stage(&runs),
        progress,
        dashboard_url,
        reason,
    }
}

/// 刷新（必要时首次创建）团队任务主卡。
///
/// 首次：reply 到 origin_message_id 生成卡片，把 message_id 存回
/// team_tasks.card_message_id。之后：update_card 原地刷新同一张卡。
///
/// 整个函数是 best-effort：任何一步失败只 warn，绝不向上传播——
/// 卡片是可观测性，不该让它的故障影响任务执行。
pub async fn sync_team_card(state: &Arc<AppState>, task_id: &str) {
    let task = match state.db.get_team_task(task_id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            tracing::warn!(%task_id, "sync_team_card: 任务不存在");
            return;
        }
        Err(e) => {
            tracing::warn!(%task_id, "sync_team_card: 读任务失败: {e:#}");
            return;
        }
    };

    // 终态与首次建卡不节流，保证立刻可见；中间态节流防撞频控
    let is_first = task
        .card_message_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .is_none();
    let is_terminal = super::is_terminal(&task.status);
    if !is_first && !is_terminal && should_throttle(task_id) {
        tracing::debug!(%task_id, "sync_team_card: 节流跳过");
        return;
    }
    if is_terminal {
        clear_throttle(task_id);
    }

    let Some(account_id) = task.account_id.as_deref().filter(|s| !s.is_empty()) else {
        tracing::warn!(%task_id, "sync_team_card: 无 account_id，跳过");
        return;
    };
    if task.chat_id.as_deref().filter(|s| !s.is_empty()).is_none() {
        tracing::warn!(%task_id, "sync_team_card: 无 chat_id，跳过");
        return;
    }

    let account = match state.db.get_feishu_account(account_id).await {
        Ok(Some(a)) if a.enabled => a,
        Ok(Some(_)) => {
            tracing::warn!(%task_id, account_id, "sync_team_card: 飞书账号已停用");
            return;
        }
        Ok(None) => {
            tracing::warn!(%task_id, account_id, "sync_team_card: 飞书账号不存在");
            return;
        }
        Err(e) => {
            tracing::warn!(%task_id, account_id, "sync_team_card: 读账号失败: {e:#}");
            return;
        }
    };

    let opts = assemble_options(state, &task).await;
    let card = build_team_stage_card(&opts);
    let api = crate::feishu::api::FeishuApi::new_archived(&account, state.db.clone());

    let topic_id = task
        .topic_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("main");
    let in_thread = topic_id != "main";

    if let Some(card_mid) = task.card_message_id.as_deref().filter(|s| !s.is_empty()) {
        if let Err(e) = api.update_card(card_mid, &card).await {
            tracing::warn!(%task_id, "sync_team_card: update_card 失败: {e:#}");
        }
        return;
    }

    // 首次：需要 origin_message_id 才能 reply
    let Some(origin) = task.origin_message_id.as_deref().filter(|s| !s.is_empty()) else {
        tracing::warn!(
            %task_id,
            "sync_team_card: card_message_id 与 origin_message_id 皆空，无法建主卡"
        );
        return;
    };

    match api.reply_card(origin, &card, in_thread).await {
        Ok(new_mid) => {
            if let Err(e) = state.db.set_team_task_card(task_id, &new_mid).await {
                tracing::warn!(%task_id, "sync_team_card: set_team_task_card 失败: {e:#}");
            } else {
                clear_throttle(task_id);
                tracing::info!(%task_id, card_mid = %new_mid, "团队任务主卡已创建");
            }
        }
        Err(e) => {
            tracing::warn!(%task_id, "sync_team_card: reply_card 失败: {e:#}");
        }
    }
}

// ---------------------------------------------------------------------------
// 单测
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn base_opts() -> TeamStageCardOptions {
        TeamStageCardOptions {
            task_no: "tsk_test_0001".into(),
            goal: "修一个 bug".into(),
            status: "running_developer".into(),
            current_role: Some("developer".into()),
            issue_key: Some("IK5MOR".into()),
            source_label: "飞书派单".into(),
            backend: "codex".into(),
            dev_rounds: 1,
            runs: vec![],
            progress: None,
            dashboard_url: Some("http://127.0.0.1:18789/#/team/tsk_test_0001".into()),
            reason: None,
        }
    }

    fn card_text(card: &Value) -> String {
        // 把 JSON 序列化后搜关键字（元素是 markdown content）
        card.to_string()
    }

    #[test]
    fn titles_and_templates_per_status() {
        let cases = [
            ("running_developer", "团队任务 · 开发 · 进行中", "blue"),
            ("running_reviewer", "团队任务 · 评审 · 进行中", "blue"),
            ("running_tester", "团队任务 · 测试 · 进行中", "blue"),
            (
                "pending_review_gate",
                "团队任务 · 开发完成 · 待进入评审",
                "orange",
            ),
            (
                "pending_dev_gate",
                "团队任务 · 评审 → 开发（打回）",
                "orange",
            ),
            ("done", "团队任务 · 已完成", "green"),
            ("failed", "团队任务 · 失败", "red"),
            ("cancelled", "团队任务 · 已取消", "grey"),
        ];
        for (status, title, template) in cases {
            let mut opts = base_opts();
            opts.status = status.into();
            if status.starts_with("running_") {
                opts.current_role = Some(status.trim_start_matches("running_").into());
            } else if super::super::is_terminal(status) {
                opts.current_role = None;
            }
            let card = build_team_stage_card(&opts);
            assert_eq!(
                card["header"]["title"]["content"].as_str(),
                Some(title),
                "status={status}"
            );
            assert_eq!(
                card["header"]["template"].as_str(),
                Some(template),
                "status={status}"
            );
        }
    }

    #[test]
    fn no_issue_key_omits_issue_label() {
        let mut opts = base_opts();
        opts.issue_key = None;
        let text = card_text(&build_team_stage_card(&opts));
        assert!(
            !text.contains("Issue"),
            "issue_key=None 时不应出现 Issue: {text}"
        );
    }

    #[test]
    fn no_dashboard_url_omits_http() {
        let mut opts = base_opts();
        opts.dashboard_url = None;
        let text = card_text(&build_team_stage_card(&opts));
        assert!(
            !text.contains("http"),
            "dashboard_url=None 时不应含 http: {text}"
        );
    }

    #[test]
    fn run_lines_finished_running_reject() {
        let mut opts = base_opts();
        opts.runs = vec![
            TeamStageRun {
                role_label: "开发".into(),
                round: 1,
                status: "finished".into(),
                verdict: Some("pass".into()),
                summary: Some("修好了".into()),
                dirty_files: Some(3),
            },
            TeamStageRun {
                role_label: "评审".into(),
                round: 1,
                status: "running".into(),
                verdict: None,
                summary: None,
                dirty_files: None,
            },
            TeamStageRun {
                role_label: "评审".into(),
                round: 1,
                status: "finished".into(),
                verdict: Some("reject".into()),
                summary: Some("漏了错误处理".into()),
                dirty_files: None,
            },
        ];
        // 上面有两个「评审 第1轮」——单测三种状态各一行，拆开更清晰
        opts.runs = vec![
            TeamStageRun {
                role_label: "开发".into(),
                round: 1,
                status: "finished".into(),
                verdict: Some("pass".into()),
                summary: Some("修好了".into()),
                dirty_files: Some(3),
            },
            TeamStageRun {
                role_label: "评审".into(),
                round: 1,
                status: "running".into(),
                verdict: None,
                summary: None,
                dirty_files: None,
            },
        ];
        let text = card_text(&build_team_stage_card(&opts));
        assert!(text.contains("✅ 开发 第1轮 · 改动 3 个文件"), "{text}");
        assert!(text.contains("🔄 评审 第1轮 · 进行中"), "{text}");

        opts.runs = vec![TeamStageRun {
            role_label: "评审".into(),
            round: 1,
            status: "finished".into(),
            verdict: Some("reject".into()),
            summary: Some("漏了错误处理".into()),
            dirty_files: None,
        }];
        let text = card_text(&build_team_stage_card(&opts));
        assert!(
            text.contains("❌ 评审 第1轮 · 打回：漏了错误处理"),
            "{text}"
        );
    }

    #[test]
    fn long_goal_truncated_by_chars_not_bytes() {
        // 600 个中文字符，截到 500，不应 panic / 切坏 UTF-8
        let long: String = "测".repeat(600);
        let mut opts = base_opts();
        opts.goal = long;
        let card = build_team_stage_card(&opts);
        let text = card_text(&card);
        // 目标区最多 500 字 + 前后缀；整卡不应含 600 个「测」
        let count = text.matches('测').count();
        assert!(count <= 500, "goal 应按字符截断到 500，实际出现 {count} 次");
        // 截断后仍是合法 JSON / 合法 UTF-8
        assert!(serde_json::to_string(&card).is_ok());
    }

    #[test]
    fn no_progress_omits_section() {
        let mut opts = base_opts();
        opts.progress = None;
        let text = card_text(&build_team_stage_card(&opts));
        assert!(
            !text.contains("当前进展"),
            "progress=None 时不应含「当前进展」: {text}"
        );
    }

    #[test]
    fn dashboard_url_format() {
        assert_eq!(
            build_dashboard_task_url("http://127.0.0.1:18789", "tsk_abc"),
            "http://127.0.0.1:18789/#/team/tsk_abc"
        );
        assert_eq!(
            build_dashboard_task_url("http://127.0.0.1:18789/", "tsk_abc"),
            "http://127.0.0.1:18789/#/team/tsk_abc"
        );
    }
}
