//! 消费 run_chat_turn 的事件流，映射为微信消息回推。

use crate::chat::EventEntry;
use crate::weixin::api::IlinkClient;
use code_agent::AgentEvent;
use hank_db::WeixinAccount;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// 进度摘要最小间隔
const PROGRESS_INTERVAL: Duration = Duration::from_secs(8);

/// spawn 一个任务消费事件流并回推微信。
pub fn spawn(
    account: WeixinAccount,
    to_user_id: String,
    context_token: String,
    rx: broadcast::Receiver<EventEntry>,
) {
    tokio::spawn(run(account, to_user_id, context_token, rx));
}

struct Progress {
    tool_counts: HashMap<String, u32>,
    changed_files: Vec<String>,
    plan_count: u32,
    last_sent: Instant,
}

impl Progress {
    fn new() -> Self {
        Self {
            tool_counts: HashMap::new(),
            changed_files: Vec::new(),
            plan_count: 0,
            last_sent: Instant::now() - PROGRESS_INTERVAL,
        }
    }

    fn record_tool(&mut self, name: &str) {
        *self.tool_counts.entry(name.to_string()).or_insert(0) += 1;
    }

    fn record_files(&mut self, paths: Vec<String>) {
        for p in paths {
            if !self.changed_files.contains(&p) {
                self.changed_files.push(p);
            }
        }
    }

    fn summary(&self) -> Option<String> {
        if self.tool_counts.is_empty() && self.changed_files.is_empty() && self.plan_count == 0 {
            return None;
        }
        let mut parts: Vec<String> = Vec::new();
        if !self.tool_counts.is_empty() {
            let mut tools: Vec<_> = self.tool_counts.iter().collect();
            tools.sort_by(|a, b| b.1.cmp(a.1));
            let s = tools
                .iter()
                .map(|(n, c)| format!("{n} ×{c}"))
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("工具 {s}"));
        }
        if !self.changed_files.is_empty() {
            let files = self.changed_files.join(", ");
            parts.push(format!("修改 {files}"));
        }
        if self.plan_count > 0 {
            parts.push(format!("计划更新 ×{}", self.plan_count));
        }
        Some(format!("执行中：{}", parts.join("；")))
    }
}

async fn run(
    account: WeixinAccount,
    to_user_id: String,
    context_token: String,
    mut rx: broadcast::Receiver<EventEntry>,
) {
    let client = IlinkClient::new();
    let send = |text: &str| {
        let client = client.clone();
        let account = account.clone();
        let to = to_user_id.clone();
        let token = context_token.clone();
        let text = text.to_string();
        async move {
            if let Err(e) = client.send_text(&account, &to, &token, &text).await {
                tracing::warn!("weixin push failed: {e:#}");
            }
        }
    };

    let mut progress = Progress::new();
    let mut final_text = String::new();
    let mut llm_calls: u32 = 0;
    let mut input_tokens: u32 = 0;
    let mut output_tokens: u32 = 0;
    let started = Instant::now();

    loop {
        let entry = match rx.recv().await {
            Ok(e) => e,
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("weixin pusher lagged by {n} events");
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => break,
        };
        match entry.event {
            AgentEvent::RunStarted { .. } => {
                send("已开始执行").await;
            }
            AgentEvent::TextDelta { text } => {
                final_text.push_str(&text);
            }
            AgentEvent::ToolStart { name, .. } => {
                progress.record_tool(&name);
                if progress.last_sent.elapsed() >= PROGRESS_INTERVAL {
                    if let Some(s) = progress.summary() {
                        send(&s).await;
                        progress.last_sent = Instant::now();
                    }
                }
            }
            AgentEvent::FileChanged { changes, .. } => {
                progress.record_files(changes.into_iter().map(|c| c.path).collect());
                if progress.last_sent.elapsed() >= PROGRESS_INTERVAL {
                    if let Some(s) = progress.summary() {
                        send(&s).await;
                        progress.last_sent = Instant::now();
                    }
                }
            }
            AgentEvent::PlanUpdated { .. } => {
                progress.plan_count += 1;
            }
            AgentEvent::AskUser { question, options, .. } => {
                let mut msg = format!("❓ {question}");
                if !options.is_empty() {
                    msg.push_str("\n选项：");
                    for (i, opt) in options.iter().enumerate() {
                        msg.push_str(&format!("\n{}. {}", i + 1, opt));
                    }
                }
                msg.push_str("\n直接回复消息作答");
                send(&msg).await;
            }
            AgentEvent::Metrics { input_tokens: it, output_tokens: ot, .. } => {
                llm_calls += 1;
                input_tokens += it;
                output_tokens += ot;
            }
            AgentEvent::RunCompleted { summary, .. } => {
                send_final(
                    &send,
                    &final_text,
                    &summary,
                    started.elapsed(),
                    llm_calls,
                    input_tokens,
                    output_tokens,
                )
                .await;
                break;
            }
            AgentEvent::RunFailed { message, .. } => {
                send(&format!("执行失败：{message}")).await;
                break;
            }
            AgentEvent::RunCancelled { .. } => {
                send("任务已取消").await;
                break;
            }
            AgentEvent::Error { message } => {
                send(&format!("出错：{message}")).await;
            }
            AgentEvent::TurnComplete => break,
            _ => {}
        }
    }
}

async fn send_final<F, Fut>(
    send: &F,
    final_text: &str,
    run_summary: &str,
    elapsed: Duration,
    llm_calls: u32,
    input_tokens: u32,
    output_tokens: u32,
) where
    F: Fn(&str) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    // 最终文本优先用 text_delta 聚合结果；为空时退回 run_completed 的 summary
    let body = if final_text.trim().is_empty() {
        run_summary.to_string()
    } else {
        final_text.trim().to_string()
    };
    let mut msg = if body.is_empty() {
        "已完成".to_string()
    } else {
        body
    };
    msg.push_str(&format!(
        "\n——\n耗时 {:.0}s · LLM 调用 {} 次 · token {}/{}",
        elapsed.as_secs_f32(),
        llm_calls,
        input_tokens,
        output_tokens
    ));
    send(&msg).await;
}
