//! 消费 run_chat_turn 的事件流，映射为飞书卡片刷新。
//!
//! 与 weixin/pusher.rs 的区别：微信是纯文本进度摘要（8s 节流），
//! 飞书用任务卡片原地刷新（2s 节流）+ AskUser 按钮确认卡片。

use crate::chat::EventEntry;
use crate::feishu::api::FeishuApi;
use crate::feishu::card::{
    build_confirm_card, build_task_card, ConfirmCardOptions, TaskCardOptions, TaskStatus,
    ThrottledCardUpdater, CARD_UPDATE_INTERVAL,
};
use crate::AppState;
use code_agent::AgentEvent;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::broadcast;

/// 最终答案文本截断上限（飞书单条文本消息不宜过长）
const MAX_FINAL_TEXT_CHARS: usize = 8000;
/// 进度上限：任务未结束时最多到 90%，100% 只给真正完成
const PROGRESS_CAP_RUNNING: u32 = 90;

#[allow(clippy::too_many_arguments)]
pub fn spawn(
    state: Arc<AppState>,
    api: FeishuApi,
    message_id: String,
    chat_id: String,
    topic_id: String,
    session_id: String,
    in_thread: bool,
    rx: broadcast::Receiver<EventEntry>,
) {
    tokio::spawn(run(
        state, api, message_id, chat_id, topic_id, session_id, in_thread, rx,
    ));
}

#[derive(Default)]
struct Progress {
    tool_counts: HashMap<String, u32>,
    changed_files: Vec<String>,
    plan_count: u32,
    activities: Vec<String>,
}

impl Progress {
    fn record_tool(&mut self, name: &str) {
        *self.tool_counts.entry(name.to_string()).or_insert(0) += 1;
        self.push_activity(format!("调用工具 {name}"));
    }

    fn record_files(&mut self, paths: Vec<String>) {
        for p in &paths {
            if !self.changed_files.contains(p) {
                self.changed_files.push(p.clone());
            }
        }
        if let Some(last) = paths.last() {
            self.push_activity(format!("修改 {last}"));
        }
    }

    fn push_activity(&mut self, s: String) {
        self.activities.push(s);
    }

    fn detail(&self) -> String {
        self.activities
            .last()
            .cloned()
            .unwrap_or_else(|| "正在执行".to_string())
    }

    /// 估算进度：无总步数可知，按活动数递增，封顶 90%
    fn percent(&self) -> u32 {
        let events: u32 = self.tool_counts.values().sum::<u32>()
            + self.changed_files.len() as u32
            + self.plan_count;
        (10 + events * 5).min(PROGRESS_CAP_RUNNING)
    }

    fn summary_line(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if !self.tool_counts.is_empty() {
            let mut tools: Vec<_> = self.tool_counts.iter().collect();
            tools.sort_by(|a, b| b.1.cmp(a.1));
            let s = tools
                .iter()
                .take(4)
                .map(|(n, c)| format!("{n} ×{c}"))
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("工具 {s}"));
        }
        if !self.changed_files.is_empty() {
            parts.push(format!("修改 {} 个文件", self.changed_files.len()));
        }
        parts.join("；")
    }
}

#[allow(clippy::too_many_arguments)]
async fn run(
    state: Arc<AppState>,
    api: FeishuApi,
    message_id: String,
    chat_id: String,
    topic_id: String,
    session_id: String,
    in_thread: bool,
    mut rx: broadcast::Receiver<EventEntry>,
) {
    // 先回一张蓝色卡片；失败则退化为纯文本模式
    let card_id = match api
        .reply_card(
            &message_id,
            &build_task_card(&TaskCardOptions {
                title: "Agent 任务".to_string(),
                status: TaskStatus::Running,
                progress: 0,
                detail: "正在启动执行引擎".to_string(),
                activities: vec![],
                footer: None,
            }),
            in_thread,
        )
        .await
    {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!("feishu: reply card failed, fallback to text: {e:#}");
            let _ = api.reply_text(&message_id, "已开始执行", in_thread).await;
            None
        }
    };

    let api_for_updates = api.clone();
    let card_id_for_updates = card_id.clone();
    let updater = ThrottledCardUpdater::new(
        move |card: Value| {
            let api = api_for_updates.clone();
            let card_id = card_id_for_updates.clone();
            async move {
                if let Some(id) = card_id {
                    if let Err(e) = api.update_card(&id, &card).await {
                        tracing::warn!("feishu: update card failed: {e:#}");
                    }
                }
            }
        },
        CARD_UPDATE_INTERVAL,
    );

    let mut progress = Progress::default();
    let mut final_text = String::new();
    let mut llm_calls: u32 = 0;
    let mut input_tokens: u32 = 0;
    let mut output_tokens: u32 = 0;
    let started = Instant::now();

    let push_running = |progress: &Progress| {
        updater.push(build_task_card(&TaskCardOptions {
            title: "Agent 任务".to_string(),
            status: TaskStatus::Running,
            progress: progress.percent(),
            detail: progress.detail(),
            activities: progress.activities.iter().rev().take(3).rev().cloned().collect(),
            footer: None,
        }));
    };

    loop {
        let entry = match rx.recv().await {
            Ok(e) => e,
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("feishu pusher lagged by {n} events");
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => break,
        };
        match entry.event {
            AgentEvent::RunStarted { .. } => {}
            AgentEvent::TextDelta { text } => {
                final_text.push_str(&text);
            }
            AgentEvent::ToolStart { name, .. } => {
                progress.record_tool(&name);
                push_running(&progress);
            }
            AgentEvent::FileChanged { changes, .. } => {
                progress.record_files(changes.into_iter().map(|c| c.path).collect());
                push_running(&progress);
            }
            AgentEvent::PlanUpdated { .. } => {
                progress.plan_count += 1;
                progress.push_activity("更新执行计划".to_string());
                push_running(&progress);
            }
            AgentEvent::AskUser { question, options, kind, .. } => {
                let is_quant = kind.as_deref().is_some_and(|k| k.starts_with("quant_confirm:"));
                let title = if is_quant { "高成本操作确认" } else { "需要你的输入" };
                let hint = if is_quant {
                    Some("点击按钮或回复文字作答；回复「确认N次」（如「确认5次」，N≤50）可批量授权本会话后续高成本操作".to_string())
                } else {
                    Some("点击按钮或直接回复消息作答".to_string())
                };
                let card = build_confirm_card(&ConfirmCardOptions {
                    title: title.to_string(),
                    question: question.clone(),
                    choices: options.clone(),
                    session_id: session_id.clone(),
                    chat_id: chat_id.clone(),
                    topic_id: topic_id.clone(),
                    hint,
                });
                if let Err(e) = api.reply_card(&message_id, &card, in_thread).await {
                    tracing::warn!("feishu: send confirm card failed: {e:#}");
                    // 降级为纯文本提问
                    let mut msg = format!("❓ {question}");
                    if !options.is_empty() {
                        msg.push_str("\n选项：");
                        for (i, opt) in options.iter().enumerate() {
                            msg.push_str(&format!("\n{}. {}", i + 1, opt));
                        }
                    }
                    let _ = api.reply_text(&message_id, &msg, in_thread).await;
                }
            }
            AgentEvent::Metrics { input_tokens: it, output_tokens: ot, .. } => {
                llm_calls += 1;
                input_tokens += it;
                output_tokens += ot;
            }
            AgentEvent::RunCompleted { summary, .. } => {
                let raw = if final_text.trim().is_empty() {
                    summary
                } else {
                    final_text.trim().to_string()
                };
                let (body, files) = extract_file_markers(&raw);
                let footer = format!(
                    "—— 耗时 {:.0}s · LLM 调用 {} 次 · token {}/{}",
                    started.elapsed().as_secs_f32(),
                    llm_calls,
                    input_tokens,
                    output_tokens
                );
                updater
                    .finish(build_task_card(&TaskCardOptions {
                        title: "Agent 任务".to_string(),
                        status: TaskStatus::Success,
                        progress: 100,
                        detail: "执行完成".to_string(),
                        activities: vec![progress.summary_line()].into_iter().filter(|s| !s.is_empty()).collect(),
                        footer: Some(footer.clone()),
                    }))
                    .await;
                send_final_text(&api, &message_id, &body, &files, &footer, in_thread).await;
                break;
            }
            AgentEvent::RunFailed { message, .. } => {
                updater
                    .finish(build_task_card(&TaskCardOptions {
                        title: "Agent 任务".to_string(),
                        status: TaskStatus::Failed,
                        progress: 0,
                        detail: message.clone(),
                        activities: vec![],
                        footer: None,
                    }))
                    .await;
                let _ = api
                    .reply_text(&message_id, &format!("执行失败：{message}"), in_thread)
                    .await;
                break;
            }
            AgentEvent::RunCancelled { .. } => {
                updater
                    .finish(build_task_card(&TaskCardOptions {
                        title: "Agent 任务".to_string(),
                        status: TaskStatus::Failed,
                        progress: progress.percent(),
                        detail: "任务已取消".to_string(),
                        activities: vec![],
                        footer: None,
                    }))
                    .await;
                let _ = api.reply_text(&message_id, "任务已取消", in_thread).await;
                break;
            }
            AgentEvent::Error { message } => {
                progress.push_activity(format!("出错：{message}"));
                push_running(&progress);
            }
            AgentEvent::TurnComplete => break,
            _ => {}
        }
    }

    updater.cancel().await;
    let _ = state;
}

/// 从最终文本中提取 [file:/路径] 标记（与 weixin/pusher.rs 同一约定）。
/// 媒体回传二期再接，当前以文本列出路径。
fn extract_file_markers(text: &str) -> (String, Vec<String>) {
    let mut files = Vec::new();
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("[file:") {
        out.push_str(&rest[..start]);
        let after = &rest[start + "[file:".len()..];
        match after.find(']') {
            Some(end) => {
                let path = after[..end].trim();
                if !path.is_empty() && !files.contains(&path.to_string()) {
                    files.push(path.to_string());
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    (out.trim().to_string(), files)
}

async fn send_final_text(
    api: &FeishuApi,
    message_id: &str,
    body: &str,
    files: &[String],
    footer: &str,
    in_thread: bool,
) {
    let mut msg = if body.trim().is_empty() {
        "已完成".to_string()
    } else {
        let mut t: String = body.trim().chars().take(MAX_FINAL_TEXT_CHARS).collect();
        if body.trim().chars().count() > MAX_FINAL_TEXT_CHARS {
            t.push_str("\n…（内容过长已截断，完整结果请到 web 端查看）");
        }
        t
    };
    if !files.is_empty() {
        msg.push_str("\n\n生成文件：");
        for f in files {
            msg.push_str(&format!("\n- {f}"));
        }
    }
    msg.push_str(&format!("\n{footer}"));
    if let Err(e) = api.reply_text(message_id, &msg, in_thread).await {
        tracing::warn!("feishu: send final text failed: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_markers_strips_and_collects() {
        let (text, files) = extract_file_markers("图已生成\n[file:/tmp/a.png]\n说明");
        assert_eq!(files, vec!["/tmp/a.png"]);
        assert_eq!(text, "图已生成\n\n说明");
    }

    #[test]
    fn progress_caps_at_90_while_running() {
        let mut p = Progress::default();
        for _ in 0..50 {
            p.record_tool("shell");
        }
        assert_eq!(p.percent(), PROGRESS_CAP_RUNNING);
    }
}
