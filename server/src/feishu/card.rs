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
    /// 闸门第一轮收尾：分析完了，在等用户点按钮。不是成功也不是失败。
    Waiting,
}

impl TaskStatus {
    fn style(self) -> (&'static str, &'static str) {
        match self {
            TaskStatus::Running => ("blue", "运行中"),
            TaskStatus::Success => ("green", "已完成"),
            TaskStatus::Failed => ("red", "执行失败"),
            TaskStatus::Waiting => ("grey", "等待确认"),
        }
    }
}

pub struct TaskCardOptions {
    /// 任务摘要标题（经 `build_task_title` 压成单行并截断），不再写死「Agent 任务」。
    pub title: String,
    pub status: TaskStatus,
    pub progress: u32,
    pub detail: String,
    pub activities: Vec<String>,
    /// 底部附加信息（耗时/token 统计等），仅完成态展示
    pub footer: Option<String>,
    /// 终态卡按钮。运行中态必须传空 vec——进度卡上出现按钮会诱导用户
    /// 在任务还没跑完时点击。
    pub actions: Vec<TaskCardAction>,
    /// 回调 value 需要的会话定位字段；运行中态照常填，只是没按钮用不到。
    pub session_id: String,
    pub chat_id: String,
    pub topic_id: String,
}

/// 卡片按钮：label 给用户看，action_id 是 feishu_card_actions 主键。
pub struct TaskCardAction {
    pub label: String,
    /// 回调 action 名：task_detail / task_suggest
    pub action: String,
    pub action_id: String,
}

/// 任务摘要 → 卡片标题。header 是 plain_text，换行/控制字符会弄坏布局，
/// 必须压成单行；超长按 chars() 截断（不能按 byte，中文会截出半个字）。
pub fn build_task_title(summary: &str) -> String {
    const MAX: usize = 24;
    let one_line: String = summary
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = one_line.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.is_empty() {
        return "Agent 任务".to_string();
    }
    let short: String = trimmed.chars().take(MAX).collect();
    if trimmed.chars().count() > MAX {
        format!("任务 · {short}…")
    } else {
        format!("任务 · {short}")
    }
}

/// 进度条字符（10 格）。`pub` 供 team_task 主卡复用，避免复制一份。
pub(crate) fn build_progress_bar(progress: u32) -> String {
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

    let mut elements = vec![json!({
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
    })];
    if !opts.actions.is_empty() {
        // 独立 action 名（task_detail / task_suggest），避免和交互单 answer 按钮混淆。
        let buttons: Vec<Value> = opts
            .actions
            .iter()
            .enumerate()
            .map(|(i, a)| {
                json!({
                    "tag": "button",
                    "text": { "tag": "plain_text", "content": a.label },
                    "type": if i == 0 { "primary" } else { "default" },
                    "behaviors": [{
                        "type": "callback",
                        "value": {
                            "action": a.action,
                            "action_id": a.action_id,
                            "session_id": opts.session_id,
                            "chat_id": opts.chat_id,
                            "topic_id": opts.topic_id,
                        }
                    }]
                })
            })
            .collect();
        elements.push(json!({ "tag": "action", "actions": buttons }));
    }

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
            "elements": elements
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

/// 多问题 ask_user 卡片入参。
pub struct MultiQuestionCardOptions {
    pub interaction_id: String,
    pub session_id: String,
    pub chat_id: String,
    pub topic_id: String,
    pub questions: Vec<code_agent::AskUserQuestion>,
    /// 已作答：题号 → 选项文案。渲染成 ✓ 行，该题不再出按钮。
    pub answered: std::collections::HashMap<String, String>,
    pub admin_url: Option<String>,
}

/// 多问题作答卡：每题一行题干 + 一组按钮（文案形如 `1A main`）；
/// 已答的题显示 `✓ 1A main`，不再出按钮。
///
/// 按钮 action 为 `answer_multi`（与一次性 `answer` 区分——点一题不能整单应答）。
pub fn build_multi_question_card(opts: &MultiQuestionCardOptions) -> Value {
    use crate::chat::flatten_question_options;

    // 合法 token 全集与落表 options / 回调白名单同源
    let flat_tokens = flatten_question_options(&opts.questions);
    let session_short: String = opts.session_id.chars().take(8).collect();
    let admin_line = opts
        .admin_url
        .as_ref()
        .map(|u| format!("\n[在 Admin 查看详情]({u})"))
        .unwrap_or_default();

    let mut elements: Vec<Value> = Vec::new();
    elements.push(json!({
        "tag": "markdown",
        "content": format!(
            "**基本信息**\n任务编号 `{}` · 会话 `{}`{}",
            opts.interaction_id, session_short, admin_line
        )
    }));

    for q in &opts.questions {
        if let Some(opt_text) = opts.answered.get(&q.id) {
            // 反查字母，token 必须落在 flatten 全集内
            let letter = q
                .options
                .iter()
                .position(|o| o == opt_text)
                .filter(|&i| i < 26)
                .map(|i| (b'A' + i as u8) as char)
                .unwrap_or('?');
            let token = format!("{}{letter}", q.id);
            debug_assert!(flat_tokens.iter().any(|t| t == &token) || letter == '?');
            elements.push(json!({
                "tag": "markdown",
                "content": format!("**{}.** {}\n✓ `{token}` {opt_text}", q.id, q.question)
            }));
        } else {
            elements.push(json!({
                "tag": "markdown",
                "content": format!("**{}.** {}", q.id, q.question)
            }));
            let buttons: Vec<Value> = q
                .options
                .iter()
                .enumerate()
                .take(26)
                .filter_map(|(i, choice)| {
                    let letter = (b'A' + i as u8) as char;
                    let choice_token = format!("{}{letter}", q.id);
                    // 只渲染 flatten 全集内的 token，保证与白名单一致
                    if !flat_tokens.iter().any(|t| t == &choice_token) {
                        return None;
                    }
                    let label = format!("{choice_token} {choice}");
                    Some(json!({
                        "tag": "button",
                        "text": { "tag": "plain_text", "content": label },
                        "type": if i == 0 { "primary" } else { "default" },
                        "behaviors": [{
                            "type": "callback",
                            "value": {
                                "action": "answer_multi",
                                "interaction_id": opts.interaction_id,
                                "question_id": q.id,
                                "choice": choice,
                                "choice_token": choice_token,
                                "session_id": opts.session_id,
                                "chat_id": opts.chat_id,
                                "topic_id": opts.topic_id,
                            }
                        }]
                    }))
                })
                .collect();
            if !buttons.is_empty() {
                elements.push(json!({
                    "tag": "action",
                    "actions": buttons
                }));
            }
        }
    }

    elements.push(json!({
        "tag": "markdown",
        "content": "*可点按钮逐题作答，或直接回复「1A 2B」一次答完*"
    }));

    json!({
        "schema": "2.0",
        "config": {
            "update_multi": true,
            "summary": { "content": "需要你的输入：多问题" }
        },
        "header": {
            "template": "orange",
            "title": { "tag": "plain_text", "content": "需要你的输入" }
        },
        "body": {
            "direction": "vertical",
            "elements": elements
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
    /// 第一轮**新增**的改动文件数（与开跑前基线的差集）。
    ///
    /// `Some(0)` = 确认没改动，不渲染警示行；`Some(n>0)` = 渲染警示行；
    /// `None` = 查不到（节点离线 / 非 git 目录），同样不渲染——不能把「不知道」
    /// 说成「没改动」，也不能拿全量脏文件冒充本轮改动。
    pub dirty_files: Option<usize>,
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
    let dirty_line = match opts.dirty_files {
        Some(n) if n > 0 => format!("\n⚠️ 第一轮已产生 {n} 个文件改动"),
        _ => String::new(),
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
            actions: vec![],
            session_id: "s1".into(),
            chat_id: "c1".into(),
            topic_id: "main".into(),
        });
        assert_eq!(card["schema"], "2.0");
        assert_eq!(card["header"]["template"], "blue");
        assert_eq!(card["config"]["update_multi"], true);
        let content = card["body"]["elements"][0]["content"].as_str().unwrap();
        assert!(content.contains("30%"));
        assert!(content.contains("步骤一"));
        // 运行中态传空 actions 时 body 只有 markdown 一个 element
        assert_eq!(card["body"]["elements"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn task_card_with_actions() {
        let card = build_task_card(&TaskCardOptions {
            title: "任务 · 完成了".into(),
            status: TaskStatus::Success,
            progress: 100,
            detail: "执行完成".into(),
            activities: vec![],
            footer: None,
            actions: vec![TaskCardAction {
                label: "查看详情".into(),
                action: "task_detail".into(),
                action_id: "act-1".into(),
            }],
            session_id: "s1".into(),
            chat_id: "c1".into(),
            topic_id: "t1".into(),
        });
        let elements = card["body"]["elements"].as_array().unwrap();
        assert_eq!(elements.len(), 2);
        let btn = &elements[1]["actions"][0];
        assert_eq!(btn["behaviors"][0]["value"]["action"], "task_detail");
        assert_eq!(btn["behaviors"][0]["value"]["action_id"], "act-1");
        assert_eq!(btn["behaviors"][0]["value"]["session_id"], "s1");
        assert_eq!(btn["type"], "primary");
    }

    #[test]
    fn build_task_title_short() {
        assert_eq!(
            build_task_title("帮我 review 登录"),
            "任务 · 帮我 review 登录"
        );
    }

    #[test]
    fn build_task_title_truncates_long_chinese() {
        // 超长中文 → 截断到 24 字 + 省略号，不能出现半个字
        let long = "帮我仔细检查一下用户登录模块的实现细节是否正确以及有没有遗漏";
        assert!(long.chars().count() > 24);
        let title = build_task_title(long);
        assert!(title.starts_with("任务 · "));
        assert!(title.ends_with('…'));
        let body = title.trim_start_matches("任务 · ").trim_end_matches('…');
        assert_eq!(body.chars().count(), 24);
        // 截断后的前缀应与原文一致（按 char 截，不是按 byte）
        let expected: String = long.chars().take(24).collect();
        assert_eq!(body, expected);
        assert!(!title.contains('\u{FFFD}'));
    }

    #[test]
    fn build_task_title_flattens_control_chars() {
        assert_eq!(
            build_task_title("第一行\n第二行\t中间"),
            "任务 · 第一行 第二行 中间"
        );
    }

    #[test]
    fn build_task_title_blank_falls_back() {
        assert_eq!(build_task_title("   \n\t  "), "Agent 任务");
        assert_eq!(build_task_title(""), "Agent 任务");
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
            dirty_files: Some(0),
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
    fn task_gate_card_hides_dirty_line_when_count_is_unknown() {
        // dirty_files=None 表示查不到（节点离线 / 非 git 目录）。
        // 不能渲染成「0 个改动」那种「已确认干净」的口径。
        let card = build_task_gate_card(&TaskGateCardOptions {
            interaction_id: "ia-unknown".into(),
            session_id: "s".into(),
            chat_id: "c".into(),
            topic_id: "t".into(),
            goal: "g".into(),
            analysis: "a".into(),
            backend: "codex".into(),
            source_label: "飞书派单".into(),
            dirty_files: None,
            admin_url: None,
        });
        let body = card["body"]["elements"].to_string();
        assert!(!body.contains("第一轮已产生"));
        assert!(!body.contains("0 个文件改动"));
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
            dirty_files: Some(3),
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

    fn sample_multi_qs() -> Vec<code_agent::AskUserQuestion> {
        vec![
            code_agent::AskUserQuestion {
                id: "1".into(),
                question: "用哪个分支？".into(),
                options: vec!["main".into(), "dev".into()],
            },
            code_agent::AskUserQuestion {
                id: "2".into(),
                question: "要跑测试吗？".into(),
                options: vec!["要".into(), "不要".into()],
            },
            code_agent::AskUserQuestion {
                id: "3".into(),
                question: "部署吗？".into(),
                options: vec!["是".into(), "否".into()],
            },
        ]
    }

    #[test]
    fn multi_question_card_all_unanswered_has_three_action_groups() {
        let card = build_multi_question_card(&MultiQuestionCardOptions {
            interaction_id: "ia-m".into(),
            session_id: "sess".into(),
            chat_id: "c".into(),
            topic_id: "t".into(),
            questions: sample_multi_qs(),
            answered: std::collections::HashMap::new(),
            admin_url: None,
        });
        let elements = card["body"]["elements"].as_array().unwrap();
        let action_count = elements.iter().filter(|e| e["tag"] == "action").count();
        assert_eq!(action_count, 3);
        // 第一个按钮 action 为 answer_multi
        let first_btn = &elements
            .iter()
            .find(|e| e["tag"] == "action")
            .unwrap()["actions"][0];
        assert_eq!(
            first_btn["behaviors"][0]["value"]["action"],
            "answer_multi"
        );
        assert_eq!(first_btn["behaviors"][0]["value"]["choice_token"], "1A");
        assert!(first_btn["text"]["content"]
            .as_str()
            .unwrap()
            .contains("1A"));
    }

    #[test]
    fn multi_question_card_partial_hides_answered_buttons() {
        let mut answered = std::collections::HashMap::new();
        answered.insert("1".into(), "main".into());
        let card = build_multi_question_card(&MultiQuestionCardOptions {
            interaction_id: "ia-m".into(),
            session_id: "sess".into(),
            chat_id: "c".into(),
            topic_id: "t".into(),
            questions: sample_multi_qs(),
            answered,
            admin_url: None,
        });
        let elements = card["body"]["elements"].as_array().unwrap();
        let action_count = elements.iter().filter(|e| e["tag"] == "action").count();
        assert_eq!(action_count, 2); // 题 2、3 仍可点
        let body = serde_json::to_string(elements).unwrap();
        assert!(body.contains('✓') || body.contains("✓"));
        assert!(body.contains("1A") || body.contains("main"));
    }

    #[test]
    fn multi_question_card_all_answered_no_actions() {
        let mut answered = std::collections::HashMap::new();
        answered.insert("1".into(), "main".into());
        answered.insert("2".into(), "要".into());
        answered.insert("3".into(), "否".into());
        let card = build_multi_question_card(&MultiQuestionCardOptions {
            interaction_id: "ia-m".into(),
            session_id: "sess".into(),
            chat_id: "c".into(),
            topic_id: "t".into(),
            questions: sample_multi_qs(),
            answered,
            admin_url: None,
        });
        let elements = card["body"]["elements"].as_array().unwrap();
        let action_count = elements.iter().filter(|e| e["tag"] == "action").count();
        assert_eq!(action_count, 0);
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
