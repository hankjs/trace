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
/// 每个按钮的 callback value 携带 action/session/chat/topic/choice，
/// 用户点击后飞书下发 card.action.trigger 事件，由 callback.rs 处理。
pub struct ConfirmCardOptions {
    pub title: String,
    pub question: String,
    /// 按钮文案列表（如 ["确认", "否"] 或 ask_user 的 options）
    pub choices: Vec<String>,
    pub session_id: String,
    pub chat_id: String,
    pub topic_id: String,
    /// 自定义回答提示
    pub hint: Option<String>,
}

pub fn build_confirm_card(opts: &ConfirmCardOptions) -> Value {
    let buttons: Vec<Value> = opts
        .choices
        .iter()
        .enumerate()
        .map(|(i, choice)| {
            // question 截断放进 value，回调后终态卡片要展示
            let question: String = opts.question.chars().take(200).collect();
            json!({
                "tag": "button",
                "text": { "tag": "plain_text", "content": choice },
                "type": if i == 0 { "primary" } else { "default" },
                "behaviors": [{
                    "type": "callback",
                    "value": {
                        "action": "answer",
                        "session_id": opts.session_id,
                        "chat_id": opts.chat_id,
                        "topic_id": opts.topic_id,
                        "choice": choice,
                        "question": question,
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
                {
                    "tag": "action",
                    "actions": buttons
                }
            ]
        }
    })
}

/// 确认完成后的终态卡片（按钮替换为结果文本，防止重复点击）。
pub fn build_confirm_done_card(title: &str, question: &str, choice: &str, operator: &str) -> Value {
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
                    "content": format!("{}\n\n**已选择：** {}（{}）", question, choice, operator)
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
            session_id: "s1".into(),
            chat_id: "c1".into(),
            topic_id: "t1".into(),
            hint: None,
        });
        let actions = &card["body"]["elements"][1]["actions"];
        assert_eq!(actions[0]["behaviors"][0]["value"]["session_id"], "s1");
        assert_eq!(actions[1]["behaviors"][0]["value"]["choice"], "否");
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
