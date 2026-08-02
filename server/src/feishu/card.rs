//! 飞书任务卡片：构建卡片内容，并把高频进度合并成低频更新。
//!
//! 复刻 docs/book/agent-os 第 05 篇的 card.ts：卡片 JSON 2.0 +
//! 两秒窗口内只提交最新状态的 ThrottledCardUpdater。

use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 进度更新节流窗口（参考文档取 2s，可按需调整）
pub const CARD_UPDATE_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Running,
    Success,
    Failed,
}

impl TaskStatus {
    fn style(self) -> (&'static str, &'static str) {
        match self {
            TaskStatus::Running => ("blue", "运行中"),
            TaskStatus::Success => ("green", "已完成"),
            TaskStatus::Failed => ("red", "执行失败"),
        }
    }
}

pub struct TaskCardOptions {
    pub title: String,
    pub status: TaskStatus,
    pub progress: u32,
    pub detail: String,
    pub activities: Vec<String>,
    /// 底部附加信息（耗时/token 统计等），仅完成态展示
    pub footer: Option<String>,
}

fn build_progress_bar(progress: u32) -> String {
    let progress = progress.min(100);
    let filled = (progress / 10) as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(10 - filled))
}

/// 构建任务进度卡片（schema 2.0）。
pub fn build_task_card(opts: &TaskCardOptions) -> Value {
    let progress = opts.progress.min(100);
    let (template, label) = opts.status.style();
    let activity_text = if opts.activities.is_empty() {
        String::new()
    } else {
        let items = opts
            .activities
            .iter()
            .map(|a| format!("- {a}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("\n\n**最近进展**\n{items}")
    };
    let footer_text = opts
        .footer
        .as_ref()
        .map(|f| format!("\n\n{f}"))
        .unwrap_or_default();

    json!({
        "schema": "2.0",
        "config": {
            "update_multi": true,
            "summary": { "content": format!("{}：{}", opts.title, label) }
        },
        "header": {
            "template": template,
            "title": { "tag": "plain_text", "content": opts.title }
        },
        "body": {
            "direction": "vertical",
            "elements": [
                {
                    "tag": "markdown",
                    "content": format!(
                        "**状态：** {}\n\n**进度：** {} {}%\n\n**当前：** {}{}{}",
                        label,
                        build_progress_bar(progress),
                        progress,
                        opts.detail,
                        activity_text,
                        footer_text
                    )
                }
            ]
        }
    })
}

/// 确认/问答卡片：question + 一组可点按钮。
///
/// 每个按钮的 callback value 携带 action/interaction_id/session/chat/topic/choice，
/// 用户点击后飞书下发 card.action.trigger 事件，由 callback.rs 按 interaction_id
/// 定位交互单（不再靠 session，避免话题重建后丢单）。
pub struct ConfirmCardOptions {
    pub title: String,
    pub question: String,
    /// 按钮文案列表（如 ["确认", "否"] 或 ask_user 的 options）
    pub choices: Vec<String>,
    /// 交互单主键，卡片展示的「任务编号」；回调按此 id 定位
    pub interaction_id: String,
    pub session_id: String,
    pub chat_id: String,
    pub topic_id: String,
    /// admin 详情深链；配置缺失时为 None，此时不渲染该行
    pub admin_url: Option<String>,
    /// 自定义回答提示
    pub hint: Option<String>,
}

pub fn build_confirm_card(opts: &ConfirmCardOptions) -> Value {
    let buttons: Vec<Value> = opts
        .choices
        .iter()
        .enumerate()
        .map(|(i, choice)| {
            json!({
                "tag": "button",
                "text": { "tag": "plain_text", "content": choice },
                "type": if i == 0 { "primary" } else { "default" },
                "behaviors": [{
                    "type": "callback",
                    "value": {
                        "action": "answer",
                        "interaction_id": opts.interaction_id,
                        "session_id": opts.session_id,
                        "chat_id": opts.chat_id,
                        "topic_id": opts.topic_id,
                        "choice": choice,
                    }
                }]
            })
        })
        .collect();

    let hint_text = opts
        .hint
        .as_ref()
        .map(|h| format!("\n\n*{h}*"))
        .unwrap_or_default();

    let session_short: String = opts.session_id.chars().take(8).collect();
    let admin_line = opts
        .admin_url
        .as_ref()
        .map(|u| format!("\n[在 Admin 查看详情]({u})"))
        .unwrap_or_default();
    // 基本信息区：两列 column_set，与部署卡片同一套飞书 schema 2.0 写法
    let info_block = json!({
        "tag": "column_set",
        "flex_mode": "none",
        "background_style": "grey",
        "columns": [
            {
                "tag": "column",
                "width": "weighted",
                "weight": 1,
                "vertical_align": "top",
                "elements": [{
                    "tag": "markdown",
                    "content": format!(
                        "**基本信息**\n任务编号 `{}`\n会话 `{}`",
                        opts.interaction_id, session_short
                    )
                }]
            },
            {
                "tag": "column",
                "width": "weighted",
                "weight": 1,
                "vertical_align": "top",
                "elements": [{
                    "tag": "markdown",
                    "content": format!("\n状态 待确认\n来源 飞书{admin_line}")
                }]
            }
        ]
    });

    json!({
        "schema": "2.0",
        "config": {
            "update_multi": true,
            "summary": { "content": format!("{}：待确认", opts.title) }
        },
        "header": {
            "template": "orange",
            "title": { "tag": "plain_text", "content": opts.title }
        },
        "body": {
            "direction": "vertical",
            "elements": [
                {
                    "tag": "markdown",
                    "content": format!("{}{}", opts.question, hint_text)
                },
                info_block,
                {
                    "tag": "action",
                    "actions": buttons
                }
            ]
        }
    })
}

/// 确认完成后的终态卡片（按钮替换为结果文本，防止重复点击）。
pub fn build_confirm_done_card(
    title: &str,
    question: &str,
    choice: &str,
    operator: &str,
    interaction_id: Option<&str>,
) -> Value {
    let id_line = interaction_id
        .filter(|id| !id.is_empty())
        .map(|id| format!("\n\n任务编号 `{id}`"))
        .unwrap_or_default();
    json!({
        "schema": "2.0",
        "config": {
            "update_multi": true,
            "summary": { "content": format!("{}：已选择 {}", title, choice) }
        },
        "header": {
            "template": "grey",
            "title": { "tag": "plain_text", "content": title }
        },
        "body": {
            "direction": "vertical",
            "elements": [
                {
                    "tag": "markdown",
                    "content": format!(
                        "{}\n\n**已选择：** {}（{}）{}",
                        question, choice, operator, id_line
                    )
                }
            ]
        }
    })
}

pub struct DeploymentCardOptions {
    pub deployment_id: String,
    pub session_id: String,
    pub chat_id: String,
    pub topic_id: String,
    pub summary: String,
    pub targets: Vec<String>,
    pub diff_stat: String,
    pub expires_at: String,
    pub approve_label: String,
}

/// 两阶段任务闸门大卡片入参（字段多，避免 too_many_arguments）。
pub struct TaskGateCardOptions {
    pub interaction_id: String,
    pub session_id: String,
    pub chat_id: String,
    pub topic_id: String,
    pub goal: String,
    pub analysis: String,
    /// 基本信息「当前角色」展示用（如 codex / claude）
    pub backend: String,
    /// 「来源」文案，如「飞书派单」
    pub source_label: String,
    /// >0 时渲染警示行：第一轮已产生改动（CLI bypass-approvals，只读靠 prompt 约束）
    pub dirty_files: usize,
    pub admin_url: Option<String>,
}

/// 飞书卡片正文长度上限；分析全文在 agent_interactions.analysis，不丢。
const TASK_GATE_ANALYSIS_CARD_LIMIT: usize = 2500;

/// 两阶段任务闸门大卡片：目标 + 基本信息 + 分析全文 + 开始修/跳过。
///
/// 按钮 callback 与 `build_confirm_card` 同构（action=answer），
/// 共用 answer_and_resume，无需 callback.rs 新分支。
pub fn build_task_gate_card(opts: &TaskGateCardOptions) -> Value {
    let analysis_chars = opts.analysis.chars().count();
    let analysis_body = if analysis_chars <= TASK_GATE_ANALYSIS_CARD_LIMIT {
        opts.analysis.clone()
    } else {
        let truncated: String = opts
            .analysis
            .chars()
            .take(TASK_GATE_ANALYSIS_CARD_LIMIT)
            .collect();
        format!("{truncated}…完整分析见 admin")
    };

    let id_short: String = opts.interaction_id.chars().take(12).collect();
    let dirty_line = if opts.dirty_files > 0 {
        format!("\n⚠️ 第一轮已产生 {} 个文件改动", opts.dirty_files)
    } else {
        String::new()
    };
    let admin_hint = opts
        .admin_url
        .as_ref()
        .map(|u| format!(" · [在 Admin 查看全文]({u})"))
        .unwrap_or_default();

    let buttons = vec![
        json!({
            "tag": "button",
            "text": { "tag": "plain_text", "content": "开始修" },
            "type": "primary",
            "behaviors": [{
                "type": "callback",
                "value": {
                    "action": "answer",
                    "interaction_id": opts.interaction_id,
                    "session_id": opts.session_id,
                    "chat_id": opts.chat_id,
                    "topic_id": opts.topic_id,
                    "choice": "开始修",
                }
            }]
        }),
        json!({
            "tag": "button",
            "text": { "tag": "plain_text", "content": "跳过" },
            "type": "default",
            "behaviors": [{
                "type": "callback",
                "value": {
                    "action": "answer",
                    "interaction_id": opts.interaction_id,
                    "session_id": opts.session_id,
                    "chat_id": opts.chat_id,
                    "topic_id": opts.topic_id,
                    "choice": "跳过",
                }
            }]
        }),
    ];

    json!({
        "schema": "2.0",
        "config": {
            "update_multi": true,
            "summary": { "content": "新任务 · 待确认是否开始修" }
        },
        "header": {
            "template": "orange",
            "title": { "tag": "plain_text", "content": "新任务 · 待确认是否开始修" }
        },
        "body": {
            "direction": "vertical",
            "elements": [
                {
                    "tag": "markdown",
                    "content": format!("**目标**\n{}", opts.goal)
                },
                {
                    "tag": "markdown",
                    "content": format!(
                        "**基本信息**\n任务编号 `{id_short}`   状态 待确认\n当前角色 {}   来源 {}{}",
                        opts.backend,
                        opts.source_label,
                        dirty_line
                    )
                },
                {
                    "tag": "hr"
                },
                {
                    "tag": "markdown",
                    "content": format!("**任务分析**\n{analysis_body}")
                },
                {
                    "tag": "markdown",
                    "content": format!("*点击下方按钮决定是否开始修{admin_hint}*")
                },
                {
                    "tag": "action",
                    "actions": buttons
                }
            ]
        }
    })
}

/// 部署审批使用独立 action，回调直接进入部署状态机，不再把“确认”交给 LLM。
pub fn build_deployment_card(opts: &DeploymentCardOptions) -> Value {
    let target_text = opts.targets.join("、");
    let diff: String = opts.diff_stat.chars().take(1800).collect();
    let button = |label: &str, decision: &str, primary: bool| {
        json!({
            "tag": "button",
            "text": { "tag": "plain_text", "content": label },
            "type": if primary { "primary" } else { "default" },
            "behaviors": [{
                "type": "callback",
                "value": {
                    "action": "deploy_approval",
                    "deployment_id": opts.deployment_id,
                    "session_id": opts.session_id,
                    "chat_id": opts.chat_id,
                    "topic_id": opts.topic_id,
                    "decision": decision,
                }
            }]
        })
    };
    json!({
        "schema": "2.0",
        "config": {
            "update_multi": true,
            "summary": { "content": format!("Trace {}：待审批", opts.approve_label) }
        },
        "header": {
            "template": "orange",
            "title": { "tag": "plain_text", "content": format!("Trace {}审批", opts.approve_label) }
        },
        "body": {
            "direction": "vertical",
            "elements": [
                {
                    "tag": "markdown",
                    "content": format!(
                        "**摘要：** {}\n\n**目标：** {}\n\n**变更：**\n```\n{}\n```\n\n**审批有效期：** {}",
                        opts.summary, target_text, diff, opts.expires_at
                    )
                },
                {
                    "tag": "action",
                    "actions": [
                        button(&opts.approve_label, "approve", true),
                        button("取消", "cancel", false)
                    ]
                }
            ]
        }
    })
}

type UpdateFn = Arc<dyn Fn(Value) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

struct UpdaterInner {
    pending: Option<Value>,
    flushing: bool,
    closed: bool,
    chain: Arc<tokio::sync::Mutex<()>>,
    update: UpdateFn,
}

/// 节流卡片更新器：窗口内无论 push 多少次，只提交最新的一张卡片。
///
/// - `push`：覆盖内存中的 pending，后台 flusher 到点只发最新值
/// - `finish`：立即提交最终卡片并关闭（等待在途更新完成，保证顺序）
/// - `cancel`：丢弃 pending 并关闭，不提交最终卡片
pub struct ThrottledCardUpdater {
    inner: Arc<Mutex<UpdaterInner>>,
    interval: Duration,
}

impl ThrottledCardUpdater {
    pub fn new<F, Fut>(update: F, interval: Duration) -> Self
    where
        F: Fn(Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let update: UpdateFn = Arc::new(move |card| Box::pin(update(card)));
        Self {
            inner: Arc::new(Mutex::new(UpdaterInner {
                pending: None,
                flushing: false,
                closed: false,
                chain: Arc::new(tokio::sync::Mutex::new(())),
                update,
            })),
            interval,
        }
    }

    pub fn push(&self, card: Value) {
        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if inner.closed {
            return;
        }
        inner.pending = Some(card);
        if !inner.flushing {
            inner.flushing = true;
            let inner_arc = Arc::clone(&self.inner);
            let interval = self.interval;
            tokio::spawn(async move {
                flusher_loop(inner_arc, interval).await;
            });
        }
    }

    /// 提交最终卡片并关闭。等待在途更新完成后再发，保证最终状态不被旧请求覆盖。
    pub async fn finish(&self, final_card: Value) {
        let (chain, update) = {
            let mut inner = match self.inner.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            inner.closed = true;
            inner.pending = None;
            (Arc::clone(&inner.chain), inner.update.clone())
        };
        let _guard = chain.lock().await;
        update(final_card).await;
    }

    /// 关闭并丢弃未发送的 pending，等待在途更新结束。
    pub async fn cancel(&self) {
        let chain = {
            let mut inner = match self.inner.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            if inner.closed {
                return;
            }
            inner.closed = true;
            inner.pending = None;
            Arc::clone(&inner.chain)
        };
        let _guard = chain.lock().await;
    }
}

async fn flusher_loop(inner: Arc<Mutex<UpdaterInner>>, interval: Duration) {
    loop {
        tokio::time::sleep(interval).await;
        let (card, chain, update) = {
            let mut guard = match inner.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            if guard.closed {
                guard.flushing = false;
                return;
            }
            match guard.pending.take() {
                Some(card) => (card, Arc::clone(&guard.chain), guard.update.clone()),
                None => {
                    // 无事可发：与 push 在同一锁内判定，push 会在锁后重新 spawn
                    guard.flushing = false;
                    return;
                }
            }
        };
        {
            let _guard = chain.lock().await;
            update(card).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn recorder() -> (UpdateFn, Arc<Mutex<Vec<Value>>>, Arc<AtomicUsize>) {
        let calls: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let count = Arc::new(AtomicUsize::new(0));
        let calls2 = calls.clone();
        let count2 = count.clone();
        let f: UpdateFn = Arc::new(move |card| {
            let calls = calls2.clone();
            let count = count2.clone();
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
                calls.lock().unwrap().push(card);
            })
        });
        (f, calls, count)
    }

    #[test]
    fn progress_bar_rendering() {
        assert_eq!(build_progress_bar(0), "░░░░░░░░░░");
        assert_eq!(build_progress_bar(50), "█████░░░░░");
        assert_eq!(build_progress_bar(100), "██████████");
        assert_eq!(build_progress_bar(150), "██████████");
    }

    #[test]
    fn task_card_structure() {
        let card = build_task_card(&TaskCardOptions {
            title: "测试任务".into(),
            status: TaskStatus::Running,
            progress: 30,
            detail: "正在读取文件".into(),
            activities: vec!["步骤一".into()],
            footer: None,
        });
        assert_eq!(card["schema"], "2.0");
        assert_eq!(card["header"]["template"], "blue");
        assert_eq!(card["config"]["update_multi"], true);
        let content = card["body"]["elements"][0]["content"].as_str().unwrap();
        assert!(content.contains("30%"));
        assert!(content.contains("步骤一"));
    }

    #[test]
    fn confirm_card_carries_callback_value() {
        let card = build_confirm_card(&ConfirmCardOptions {
            title: "确认".into(),
            question: "执行高成本操作？".into(),
            choices: vec!["确认".into(), "否".into()],
            interaction_id: "ia-123".into(),
            session_id: "s1abcdef".into(),
            chat_id: "c1".into(),
            topic_id: "t1".into(),
            admin_url: Some("https://admin.example/admin/interactions/ia-123".into()),
            hint: None,
        });
        // elements: [question, info_block, actions]
        let actions = &card["body"]["elements"][2]["actions"];
        assert_eq!(
            actions[0]["behaviors"][0]["value"]["session_id"],
            "s1abcdef"
        );
        assert_eq!(
            actions[0]["behaviors"][0]["value"]["interaction_id"],
            "ia-123"
        );
        assert_eq!(actions[1]["behaviors"][0]["value"]["choice"], "否");
        // 不应再塞 question 进 callback payload
        assert!(actions[0]["behaviors"][0]["value"]
            .get("question")
            .is_none());
        let info = card["body"]["elements"][1].to_string();
        assert!(info.contains("ia-123"));
        assert!(info.contains("s1abcdef")); // session 前 8 位（本例即全文）
        assert!(info.contains("Admin"));
    }

    #[test]
    fn confirm_card_hides_admin_link_when_url_missing() {
        let card = build_confirm_card(&ConfirmCardOptions {
            title: "确认".into(),
            question: "q?".into(),
            choices: vec!["确认".into()],
            interaction_id: "ia-9".into(),
            session_id: "sess".into(),
            chat_id: "c".into(),
            topic_id: "t".into(),
            admin_url: None,
            hint: None,
        });
        let info = card["body"]["elements"][1].to_string();
        assert!(info.contains("ia-9"));
        assert!(!info.contains("Admin"));
        assert!(!info.contains("/#/"));
    }

    #[test]
    fn confirm_done_card_shows_interaction_id() {
        let card = build_confirm_done_card("确认", "q?", "确认", "你", Some("ia-1"));
        let content = card["body"]["elements"][0]["content"].as_str().unwrap();
        assert!(content.contains("ia-1"));
        assert!(content.contains("已选择"));
    }

    #[test]
    fn task_gate_card_carries_answer_callbacks_and_fields() {
        let card = build_task_gate_card(&TaskGateCardOptions {
            interaction_id: "ia-gate-abcdef012345".into(),
            session_id: "sess-1".into(),
            chat_id: "chat-1".into(),
            topic_id: "topic-1".into(),
            goal: "修一下登录 bug".into(),
            analysis: "## 目标\nok\n## 范围\nok\n## 疑似改动点\nok\n## 风险\nok".into(),
            backend: "codex".into(),
            source_label: "飞书派单".into(),
            dirty_files: 0,
            admin_url: Some("https://admin.example/admin/interactions/ia-gate-abcdef012345".into()),
        });
        let body = card["body"]["elements"].to_string();
        assert!(body.contains("ia-gate-abcd")); // 前 12 位
        assert!(body.contains("修一下登录 bug"));
        assert!(body.contains("codex"));
        assert!(body.contains("飞书派单"));
        assert!(!body.contains("第一轮已产生"));

        // 最后一个 element 是 action 按钮
        let actions = card["body"]["elements"].as_array().unwrap().last().unwrap()["actions"]
            .as_array()
            .unwrap();
        assert_eq!(actions.len(), 2);
        let v0 = &actions[0]["behaviors"][0]["value"];
        let v1 = &actions[1]["behaviors"][0]["value"];
        assert_eq!(v0["action"], "answer");
        assert_eq!(v0["choice"], "开始修");
        assert_eq!(v0["interaction_id"], "ia-gate-abcdef012345");
        assert_eq!(v1["action"], "answer");
        assert_eq!(v1["choice"], "跳过");
        assert_eq!(actions[1]["type"], "default");
    }

    #[test]
    fn task_gate_card_shows_dirty_warning_and_truncates_analysis() {
        let long_analysis: String = "甲".repeat(2600);
        let card = build_task_gate_card(&TaskGateCardOptions {
            interaction_id: "ia-dirty".into(),
            session_id: "s".into(),
            chat_id: "c".into(),
            topic_id: "t".into(),
            goal: "g".into(),
            analysis: long_analysis,
            backend: "claude".into(),
            source_label: "飞书派单".into(),
            dirty_files: 3,
            admin_url: Some("https://admin.example/admin/interactions/ia-dirty".into()),
        });
        let body = card["body"]["elements"].to_string();
        assert!(body.contains("第一轮已产生 3 个文件改动"));
        assert!(body.contains("完整分析见 admin"));
        // 截断后正文不应仍含满 2600 字
        let analysis_el = card["body"]["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|el| {
                el["content"]
                    .as_str()
                    .is_some_and(|c| c.starts_with("**任务分析**"))
            })
            .unwrap();
        let content = analysis_el["content"].as_str().unwrap();
        assert!(content.chars().count() < 2600 + 20);
    }

    #[tokio::test(start_paused = true)]
    async fn throttle_merges_rapid_pushes() {
        let (update, calls, _count) = recorder();
        let updater = ThrottledCardUpdater::new(move |card| update(card), Duration::from_secs(2));
        for i in 0..5 {
            updater.push(json!({ "n": i }));
        }
        // 窗口内 5 次 push 只应提交最后一次
        tokio::task::yield_now().await; // 让 flusher 任务先注册
        tokio::time::advance(Duration::from_secs(3)).await;
        for _ in 0..10 {
            tokio::task::yield_now().await; // 驱动 flusher 完成取件与提交
        }
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["n"], 4);
    }

    #[tokio::test(start_paused = true)]
    async fn finish_flushes_final_immediately() {
        let (update, calls, _count) = recorder();
        let updater = ThrottledCardUpdater::new(move |card| update(card), Duration::from_secs(2));
        updater.push(json!({ "n": 1 }));
        updater.finish(json!({ "final": true })).await;
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["final"], true);
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_drops_pending() {
        let (update, calls, _count) = recorder();
        let updater = ThrottledCardUpdater::new(move |card| update(card), Duration::from_secs(2));
        updater.push(json!({ "n": 1 }));
        tokio::task::yield_now().await; // 让 flusher 进入睡眠
        updater.cancel().await;
        tokio::time::advance(Duration::from_secs(5)).await;
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(calls.lock().unwrap().is_empty());
    }
}
