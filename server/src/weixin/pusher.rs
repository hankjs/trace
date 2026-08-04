//! 消费 run_chat_turn 的事件流，映射为微信消息回推。

use crate::chat::EventEntry;
// 与飞书 pusher 共用滞后补偿：Lagged / 静默都回 EventBuffer 补齐，
// 否则终态事件被丢掉后这个循环会永远等下去，任务不汇报完成。
use crate::task_state::{drain_buffer, next_event, Incoming, ProgressSnapshot};
use crate::weixin::api::IlinkClient;
use crate::AppState;
use anyhow::{anyhow, Result};
use code_agent::AgentEvent;
use hank_db::WeixinAccount;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// 进度摘要最小间隔
const PROGRESS_INTERVAL: Duration = Duration::from_secs(8);
/// 进度上限：任务未结束时最多到 90%
const PROGRESS_CAP_RUNNING: u32 = 90;
/// 单次任务最多回传的媒体文件数
const MAX_MEDIA_FILES: usize = 5;
/// 单个媒体文件大小上限（base64 回传/微信 CDN 均有成本）
const MAX_MEDIA_BYTES: usize = 20 * 1024 * 1024;

/// spawn 一个任务消费事件流并回推微信。
pub fn spawn(
    state: Arc<AppState>,
    account: WeixinAccount,
    to_user_id: String,
    context_token: String,
    session_id: String,
    rx: broadcast::Receiver<EventEntry>,
) {
    tokio::spawn(run(
        state,
        account,
        to_user_id,
        context_token,
        session_id,
        rx,
    ));
}

struct Progress {
    tool_counts: HashMap<String, u32>,
    changed_files: Vec<String>,
    plan_count: u32,
    last_sent: Instant,
    /// 最近活动，供"进度怎样了"这类询问读取
    activities: Vec<String>,
}

impl Progress {
    fn new() -> Self {
        Self {
            tool_counts: HashMap::new(),
            changed_files: Vec::new(),
            plan_count: 0,
            last_sent: Instant::now() - PROGRESS_INTERVAL,
            activities: Vec::new(),
        }
    }

    fn record_tool(&mut self, name: &str) {
        *self.tool_counts.entry(name.to_string()).or_insert(0) += 1;
        self.activities.push(format!("调用工具 {name}"));
    }

    fn record_files(&mut self, paths: Vec<String>) {
        if let Some(last) = paths.last() {
            self.activities.push(format!("修改 {last}"));
        }
        for p in paths {
            if !self.changed_files.contains(&p) {
                self.changed_files.push(p);
            }
        }
    }

    /// 当前动作，与飞书卡片的"当前："同义
    fn detail(&self) -> String {
        self.activities
            .last()
            .cloned()
            .unwrap_or_else(|| "正在执行".to_string())
    }

    /// 估算进度：无总步数可知，按活动数递增，运行中封顶 90%
    fn percent(&self) -> u32 {
        let events: u32 = self.tool_counts.values().sum::<u32>()
            + self.changed_files.len() as u32
            + self.plan_count;
        (10 + events * 5).min(PROGRESS_CAP_RUNNING)
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
    state: Arc<AppState>,
    account: WeixinAccount,
    to_user_id: String,
    context_token: String,
    session_id: String,
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

    // 进度快照：让"进度怎样了"不必等下一次 8s 摘要就能读到当前状态。
    // 直接 await 而不是 spawn，保证写入顺序（spawn 会让旧快照覆盖新快照）。
    let publish = |progress: &Progress| {
        let snapshot = ProgressSnapshot {
            percent: progress.percent(),
            detail: progress.detail(),
            activities: progress
                .activities
                .iter()
                .rev()
                .take(3)
                .rev()
                .cloned()
                .collect(),
            started_at: started,
        };
        let state = state.clone();
        let session_id = session_id.clone();
        async move {
            state.tasks.set_progress(&session_id, snapshot).await;
        }
    };

    publish(&progress).await;

    let mut queue: VecDeque<EventEntry> = VecDeque::new();
    let mut last_id: u64 = 0;
    let mut finished = false;

    'outer: while !finished {
        if queue.is_empty() {
            match next_event(&mut rx).await {
                Incoming::Event(entry) => queue.push_back(entry),
                Incoming::Lagged(n) => {
                    tracing::warn!(
                        session_id = %session_id,
                        "weixin pusher lagged by {n} events, recovering from buffer"
                    );
                    let (missed, _) = drain_buffer(&state, &session_id, last_id).await;
                    queue.extend(missed);
                }
                Incoming::Idle | Incoming::Closed => {
                    let (missed, completed) = drain_buffer(&state, &session_id, last_id).await;
                    queue.extend(missed);
                    if queue.is_empty() && completed {
                        // 事件流结束却没见到终态事件：必须给个交代，不能静默挂着。
                        tracing::warn!(
                            session_id = %session_id,
                            "weixin pusher: event stream ended without terminal event"
                        );
                        if final_text.trim().is_empty() {
                            send("任务已结束，但没有收到完成事件，请用 /status 确认结果").await;
                        } else {
                            let (body, _) = extract_file_markers(final_text.trim());
                            send(&format!("{body}\n\n（未收到完成事件，以上为已产出内容）")).await;
                        }
                        break 'outer;
                    }
                    continue;
                }
            }
        }

        while let Some(entry) = queue.pop_front() {
            // 补读与实时流可能重叠，按 id 去重
            if entry.id <= last_id {
                continue;
            }
            last_id = entry.id;
            match entry.event {
                AgentEvent::RunStarted { .. } => {
                    send("已开始执行").await;
                }
                AgentEvent::TextDelta { text } => {
                    final_text.push_str(&text);
                }
                AgentEvent::ToolStart { name, .. } => {
                    progress.record_tool(&name);
                    publish(&progress).await;
                    if progress.last_sent.elapsed() >= PROGRESS_INTERVAL {
                        if let Some(s) = progress.summary() {
                            send(&s).await;
                            progress.last_sent = Instant::now();
                        }
                    }
                }
                AgentEvent::FileChanged { changes, .. } => {
                    progress.record_files(changes.into_iter().map(|c| c.path).collect());
                    publish(&progress).await;
                    if progress.last_sent.elapsed() >= PROGRESS_INTERVAL {
                        if let Some(s) = progress.summary() {
                            send(&s).await;
                            progress.last_sent = Instant::now();
                        }
                    }
                }
                AgentEvent::PlanUpdated { .. } => {
                    progress.plan_count += 1;
                    progress.activities.push("更新执行计划".to_string());
                    publish(&progress).await;
                }
                AgentEvent::AskUser {
                    question, options, ..
                } => {
                    let mut msg = format!("❓ {question}");
                    if !options.is_empty() {
                        msg.push_str("\n选项：");
                        for (i, opt) in options.iter().enumerate() {
                            msg.push_str(&format!("\n{}. {}", i + 1, opt));
                        }
                    }
                    msg.push_str("\n直接回复消息作答");
                    send(&msg).await;
                    // ask_user 期间 run 暂停，进度停在等人作答
                    progress.activities.push("等待用户确认".to_string());
                    publish(&progress).await;
                }
                AgentEvent::Metrics {
                    input_tokens: it,
                    output_tokens: ot,
                    ..
                } => {
                    llm_calls += 1;
                    input_tokens += it;
                    output_tokens += ot;
                }
                AgentEvent::RunCompleted { summary, .. } => {
                    // 最终文本优先用 text_delta 聚合结果；为空时退回 run_completed 的 summary
                    let raw = if final_text.trim().is_empty() {
                        summary
                    } else {
                        final_text.trim().to_string()
                    };
                    let (body, files) = extract_file_markers(&raw);
                    send_final(
                        &send,
                        &body,
                        started.elapsed(),
                        llm_calls,
                        input_tokens,
                        output_tokens,
                    )
                    .await;
                    send_media_files(&client, &account, &to_user_id, &context_token, &files, &send)
                        .await;
                    finished = true;
                    break;
                }
                AgentEvent::RunFailed { message, .. } => {
                    send(&format!("执行失败：{message}")).await;
                    finished = true;
                    break;
                }
                AgentEvent::RunCancelled { .. } => {
                    send("任务已取消").await;
                    finished = true;
                    break;
                }
                AgentEvent::Error { message } => {
                    send(&format!("出错：{message}")).await;
                }
                // TurnComplete 在终态事件之后发出。若因滞后先看到它，
                // 先把 buffer 里剩下的事件补完再收尾，避免丢掉 RunCompleted 的正文。
                AgentEvent::TurnComplete => {
                    let (missed, _) = drain_buffer(&state, &session_id, last_id).await;
                    if missed.is_empty() {
                        finished = true;
                        break;
                    }
                    queue.extend(missed);
                }
                _ => {}
            }
        }
    }

    state.tasks.clear_progress(&session_id).await;
}

/// 从最终文本中提取 [file:/路径] 标记，返回去掉标记后的文本和文件路径列表。
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
                if !path.is_empty()
                    && files.len() < MAX_MEDIA_FILES
                    && !files.contains(&path.to_string())
                {
                    files.push(path.to_string());
                }
                rest = &after[end + 1..];
            }
            None => {
                // 未闭合的标记按原文保留
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    (out.trim().to_string(), files)
}


/// 读取待发送文件的字节（一律 server 本地：agent 与截图都在 server 上跑）。
async fn load_media_bytes(path: &str) -> Result<Vec<u8>> {
    Ok(tokio::fs::read(path).await?)
}

/// 逐个发送标记的媒体文件；失败时降级为文本提示。
async fn send_media_files<F, Fut>(
    client: &IlinkClient,
    account: &WeixinAccount,
    to_user_id: &str,
    context_token: &str,
    files: &[String],
    send: &F,
) where
    F: Fn(&str) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    for path in files {
        let file_name = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());
        let result = async {
            let bytes = load_media_bytes(path).await?;
            if bytes.is_empty() {
                return Err(anyhow!("文件为空"));
            }
            if bytes.len() > MAX_MEDIA_BYTES {
                return Err(anyhow!("文件超过 {}MB 上限", MAX_MEDIA_BYTES / 1024 / 1024));
            }
            client
                .send_media(account, to_user_id, context_token, &file_name, &bytes)
                .await
        }
        .await;
        if let Err(e) = result {
            tracing::warn!(path, "weixin: send media failed: {e:#}");
            send(&format!("文件 {file_name} 发送失败：{e:#}")).await;
        }
    }
}

async fn send_final<F, Fut>(
    send: &F,
    body: &str,
    elapsed: Duration,
    llm_calls: u32,
    input_tokens: u32,
    output_tokens: u32,
) where
    F: Fn(&str) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let mut msg = if body.trim().is_empty() {
        "已完成".to_string()
    } else {
        body.trim().to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_markers_strips_and_collects() {
        let (text, files) =
            extract_file_markers("图已生成\n[file:/tmp/a.png]\n[file: /data/b.pdf ] 说明");
        assert_eq!(files, vec!["/tmp/a.png", "/data/b.pdf"]);
        assert_eq!(text, "图已生成\n\n 说明");
    }

    #[test]
    fn extract_markers_keeps_unclosed() {
        let (text, files) = extract_file_markers("结果 [file:/tmp/x.png");
        assert!(files.is_empty());
        assert_eq!(text, "结果 [file:/tmp/x.png");
    }

}
