//! 消费 run_chat_turn 的事件流，映射为 handy 进度卡片 + 人工闸门。
//!
//! 与 weixin pusher 同一个事件来源（EventBuffer 的 broadcast）；区别是
//! handy 是卡片协议：run 开始建卡，进行中 2s 节流原地刷新，终态立即穿透。
//! AskUser 事件 → handy 交互单（用户在 handy 网页点按钮作答），应答主要走
//! webhook 回推，这里另起 30s 轮询兜底（handy webhook 重试 3 次后放弃）。

use crate::chat::EventEntry;
use crate::handy::client::{CardUpdate, HandyApi, OpenInteraction};
use crate::handy::webhook;
// 与 weixin pusher 共用滞后补偿：Lagged / 静默都回 EventBuffer 补齐，
// 否则终态事件被丢掉后这个循环会永远等下去。
use crate::task_state::{drain_buffer, next_event, Incoming};
use crate::AppState;
use code_agent::{AgentEvent, AskUserQuestion};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// 卡片刷新最小间隔（handy /cards 端点不节流，接入方自控频率）
const PUSH_INTERVAL: Duration = Duration::from_secs(2);
/// 进度上限：任务未结束时最多到 90%
const PROGRESS_CAP_RUNNING: u32 = 90;
/// 兜底轮询间隔（webhook 丢失时兜底回收应答）
const POLL_INTERVAL: Duration = Duration::from_secs(30);
/// 兜底轮询上限：24 小时（30s × 2880），防止交互单永远 pending 时轮询泄漏
const MAX_POLL_TICKS: u32 = 2880;
/// handy topic title 上限（handy 侧截断 255，这里留余量）
const TOPIC_TITLE_CHARS: usize = 200;

/// spawn 一个任务消费事件流并回推 handy。
pub fn spawn(
    state: Arc<AppState>,
    api: HandyApi,
    session_id: String,
    task_title: String,
    rx: broadcast::Receiver<EventEntry>,
) {
    tokio::spawn(run(state, api, session_id, task_title, rx));
}

/// trace 交互单 kind → handy kind（白名单 confirm|ask_user|task_gate）。
/// quant_confirm 等确认类闸门统一落到 confirm。
fn map_interaction_kind(trace_kind: &str) -> &'static str {
    match trace_kind {
        "ask_user" => "ask_user",
        "task_gate" => "task_gate",
        _ => "confirm",
    }
}

struct Progress {
    tool_counts: HashMap<String, u32>,
    changed_files: Vec<String>,
    plan_count: u32,
    /// 最近活动，新的在后；推卡片时只取尾部（handy 只留后 8 条）
    activities: Vec<String>,
}

impl Progress {
    fn new() -> Self {
        Self {
            tool_counts: HashMap::new(),
            changed_files: Vec::new(),
            plan_count: 0,
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

    /// 当前动作，卡片 detail 用
    fn detail(&self) -> String {
        self.activities
            .last()
            .cloned()
            .unwrap_or_else(|| "正在执行".to_string())
    }

    /// 估算进度：无总步数可知，按活动数递增，运行中封顶 90%
    fn percent(&self) -> u32 {
        let events: u32 =
            self.tool_counts.values().sum::<u32>() + self.changed_files.len() as u32
                + self.plan_count;
        (10 + events * 5).min(PROGRESS_CAP_RUNNING)
    }

    /// 卡片 activities 字段：只带尾部 8 条
    fn card_activities(&self) -> Vec<String> {
        self.activities
            .iter()
            .rev()
            .take(8)
            .rev()
            .cloned()
            .collect()
    }
}

async fn run(
    state: Arc<AppState>,
    api: HandyApi,
    session_id: String,
    task_title: String,
    mut rx: broadcast::Receiver<EventEntry>,
) {
    // 建话题（按 external_id=session_id 幂等；title 只在新建时写入）。
    // 话题都建不起来说明 handy 不通，后面每一步都会失败，直接退。
    let topic_title: String = task_title.chars().take(TOPIC_TITLE_CHARS).collect();
    let topic_id = match api.upsert_topic(&session_id, &topic_title).await {
        Ok(t) => t.topic_id,
        Err(e) => {
            tracing::warn!(session_id = %session_id, "handy upsert_topic 失败，本轮不推送: {e:#}");
            return;
        }
    };
    // 登记 话题=会话 映射：入站 webhook（message.created）靠映射定位会话；
    // 映射缺失时 router 也会自动建会话登记，这里是更常见的直挂路径。
    // upsert 语义；失败不阻断下行推送。
    if let Err(e) = state.db.set_handy_chat(&topic_id, &session_id).await {
        tracing::warn!(session_id = %session_id, "handy 登记话题映射失败: {e:#}");
    }

    let mut card = CardContext {
        api: api.clone(),
        topic_id,
        card_id: None,
        title: task_title,
        last_push: Instant::now() - PUSH_INTERVAL,
    };
    card.push("running", Some(0), Some("已开始执行"), None, None)
        .await;

    let mut progress = Progress::new();
    let mut final_text = String::new();
    let mut llm_calls: u32 = 0;
    let mut input_tokens: u32 = 0;
    let mut output_tokens: u32 = 0;
    let started = Instant::now();
    // ask_user 暂停期间为 true：其后的终态事件只收尾循环，不把卡片翻成 success
    let mut waiting_for_user = false;

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
                        "handy pusher lagged by {n} events, recovering from buffer"
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
                            "handy pusher: event stream ended without terminal event"
                        );
                        if !waiting_for_user {
                            card.push(
                                "failed",
                                None,
                                Some("任务已结束，但没有收到完成事件"),
                                None,
                                None,
                            )
                            .await;
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
                    card.throttled_push(&progress, None).await;
                }
                AgentEvent::TextDelta { text } => {
                    final_text.push_str(&text);
                }
                AgentEvent::ToolStart { name, .. } => {
                    progress.record_tool(&name);
                    card.throttled_push(&progress, None).await;
                }
                AgentEvent::FileChanged { changes, .. } => {
                    progress.record_files(changes.into_iter().map(|c| c.path).collect());
                    card.throttled_push(&progress, None).await;
                }
                AgentEvent::PlanUpdated { .. } => {
                    progress.plan_count += 1;
                    progress.activities.push("更新执行计划".to_string());
                    card.throttled_push(&progress, None).await;
                }
                AgentEvent::AskUser {
                    question,
                    options,
                    kind,
                    questions,
                    ..
                } => {
                    waiting_for_user = true;
                    progress.activities.push("等待用户确认".to_string());
                    open_handy_gate(
                        &state,
                        &api,
                        &card.topic_id,
                        &session_id,
                        &question,
                        &options,
                        kind.as_deref(),
                        &questions,
                    )
                    .await;
                    // 卡片转 waiting（handy 交互单也会自动建一张 waiting 卡）
                    let detail = question.lines().next().unwrap_or("等待用户确认");
                    card.push("waiting", None, Some(detail), Some(progress.card_activities()), None)
                        .await;
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
                    if waiting_for_user {
                        // ask_user 暂停的收尾：卡片保持 waiting，由应答后的新一轮接管
                        finished = true;
                        break;
                    }
                    // 最终文本优先用 text_delta 聚合结果；为空时退回 run_completed 的 summary
                    let body = if final_text.trim().is_empty() {
                        summary.trim().to_string()
                    } else {
                        final_text.trim().to_string()
                    };
                    let stats = format!(
                        "\n——\n耗时 {:.0}s · LLM 调用 {} 次 · token {}/{}",
                        started.elapsed().as_secs_f32(),
                        llm_calls,
                        input_tokens,
                        output_tokens
                    );
                    let full_text = if body.is_empty() {
                        stats.trim_start().to_string()
                    } else {
                        format!("{body}{stats}")
                    };
                    card.push(
                        "success",
                        Some(100),
                        Some("已完成"),
                        Some(progress.card_activities()),
                        Some(full_text),
                    )
                    .await;
                    finished = true;
                    break;
                }
                AgentEvent::RunFailed { message, .. } => {
                    if !waiting_for_user {
                        card.push(
                            "failed",
                            None,
                            Some(&format!("执行失败：{message}")),
                            Some(progress.card_activities()),
                            None,
                        )
                        .await;
                    }
                    finished = true;
                    break;
                }
                AgentEvent::RunCancelled { .. } => {
                    if !waiting_for_user {
                        card.push("failed", None, Some("任务已取消"), None, None)
                            .await;
                    }
                    finished = true;
                    break;
                }
                AgentEvent::Error { message } => {
                    progress.activities.push(format!("出错：{message}"));
                    card.throttled_push(&progress, None).await;
                }
                // TurnComplete 在终态事件之后发出。若因滞后先看到它，
                // 先把 buffer 里剩下的事件补完再收尾，避免丢掉 RunCompleted 的正文。
                AgentEvent::TurnComplete => {
                    let (missed, _) = drain_buffer(&state, &session_id, last_id).await;
                    if missed.is_empty() {
                        if waiting_for_user {
                            // ask_user 暂停的 run 没有终态事件，TurnComplete 即收尾
                            finished = true;
                            break;
                        }
                        // 没等到终态事件就结束了：同 Idle/Closed 的兜底
                        card.push(
                            "failed",
                            None,
                            Some("任务已结束，但没有收到完成事件"),
                            None,
                            None,
                        )
                        .await;
                        finished = true;
                        break;
                    }
                    queue.extend(missed);
                }
                _ => {}
            }
        }
    }
}

/// 卡片上下文：持 handy card_id，节流原地刷新。
struct CardContext {
    api: HandyApi,
    topic_id: String,
    card_id: Option<String>,
    title: String,
    last_push: Instant,
}

impl CardContext {
    /// 立即推一次卡片更新（终态 / 状态切换用，不节流）。
    async fn push(
        &mut self,
        status: &str,
        progress: Option<u32>,
        detail: Option<&str>,
        activities: Option<Vec<String>>,
        full_text: Option<String>,
    ) {
        let update = CardUpdate {
            topic_id: self.topic_id.clone(),
            card_id: self.card_id.clone(),
            // title 只在建卡时传：handy 侧空串=保留旧值，后续更新不必重复
            title: if self.card_id.is_none() {
                Some(self.title.clone())
            } else {
                None
            },
            status: status.to_string(),
            progress,
            detail: detail.map(|s| s.to_string()),
            activities,
            full_text,
        };
        match self.api.upsert_card(&update).await {
            Ok(card_id) => {
                self.card_id = Some(card_id);
                self.last_push = Instant::now();
            }
            Err(e) => tracing::warn!("handy upsert_card failed: {e:#}"),
        }
    }

    /// 进行中的节流刷新：距上次推送不足 PUSH_INTERVAL 就跳过（中间态合并）。
    async fn throttled_push(&mut self, progress: &Progress, status: Option<&str>) {
        if self.last_push.elapsed() < PUSH_INTERVAL {
            return;
        }
        self.push(
            status.unwrap_or("running"),
            Some(progress.percent()),
            Some(&progress.detail()),
            Some(progress.card_activities()),
            None,
        )
        .await;
    }
}

/// AskUser 事件 → handy 交互单 + 兜底轮询。
///
/// 顺序保证：chat.rs 的事件 forwarder 先把交互单落库再 push 到 event_buffers，
/// 所以这里 latest_pending_interaction 一定能取到刚落库的单。
#[allow(clippy::too_many_arguments)]
async fn open_handy_gate(
    state: &Arc<AppState>,
    api: &HandyApi,
    topic_id: &str,
    session_id: &str,
    question: &str,
    options: &[String],
    kind: Option<&str>,
    questions: &[AskUserQuestion],
) {
    let Some(trace_interaction) = state
        .db
        .latest_pending_interaction(session_id)
        .await
        .ok()
        .flatten()
    else {
        tracing::warn!(session_id = %session_id, "handy 闸门：未找到已落库的交互单，跳过");
        return;
    };

    // quant_confirm:xxx → confirm；ask_user / task_gate 原样映射
    let handy_kind = map_interaction_kind(&trace_interaction.kind);
    let _ = kind; // 事件上的 kind 已由落库逻辑解析进交互单，以库为准
    // resume_ref 只放 trace_interaction_id 一个键：handy 会注入覆盖
    // final_answer / partial_answers，这两个键名禁用。
    let resume_ref = serde_json::json!({"trace_interaction_id": trace_interaction.id});
    // 多问题透传 questions（字段 {id,question,options} 与 handy 契约对齐，
    // handy 端逐题作答、答完合成 final answer 回推）；单问题走 options 按钮。
    // options 以事件为准；库里的 options 列是扁平合法答案集，不适合当按钮。
    let req = OpenInteraction {
        topic_id,
        kind: handy_kind,
        question,
        title: Some(&trace_interaction.title),
        options: if questions.is_empty() && !options.is_empty() {
            Some(options.to_vec())
        } else {
            None
        },
        questions: if questions.is_empty() {
            None
        } else {
            serde_json::to_value(questions).ok()
        },
        resume_ref: Some(resume_ref),
    };

    match api.open_interaction(&req).await {
        Ok(interaction) => {
            tracing::info!(
                session_id = %session_id,
                handy_interaction_id = %interaction.id,
                trace_interaction_id = %trace_interaction.id,
                "handy 交互单已打开"
            );
            spawn_answer_poller(
                state.clone(),
                api.clone(),
                interaction.id,
                trace_interaction.id.clone(),
            );
        }
        Err(e) => {
            tracing::warn!(session_id = %session_id, "handy open_interaction 失败: {e:#}")
        }
    }
}

/// webhook 丢应答时的兜底：每 30s 轮询 handy 交互单，直到离开 pending。
/// answered → 走与 webhook 共用的处理函数（原子应答天然幂等，重复无害）；
/// expired/cancelled/superseded 等 → 停止。trace 侧单已被其他路径应答也停止。
fn spawn_answer_poller(
    state: Arc<AppState>,
    api: HandyApi,
    handy_interaction_id: String,
    trace_interaction_id: String,
) {
    tokio::spawn(async move {
        for _ in 0..MAX_POLL_TICKS {
            tokio::time::sleep(POLL_INTERVAL).await;

            // trace 侧已离开 pending（webhook / admin 已应答，或已过期取消）→ 收工
            match state.db.get_interaction(&trace_interaction_id).await {
                Ok(Some(row)) if row.status == "pending" => {}
                _ => return,
            }

            match api.get_interaction(&handy_interaction_id).await {
                Ok(interaction) => match interaction.status.as_str() {
                    "pending" => continue,
                    "answered" => {
                        let answer = interaction.answer.unwrap_or_default();
                        if answer.is_empty() {
                            tracing::warn!(
                                handy_interaction_id = %handy_interaction_id,
                                "handy 交互单已应答但 answer 为空，停止轮询"
                            );
                        } else {
                            webhook::answer_trace_interaction(
                                &state,
                                &trace_interaction_id,
                                &answer,
                            )
                            .await;
                        }
                        return;
                    }
                    // expired / cancelled / superseded / done / failed：闸门已关，收工
                    _ => return,
                },
                Err(e) => {
                    tracing::warn!(
                        handy_interaction_id = %handy_interaction_id,
                        "handy 兜底轮询失败（下轮重试）: {e:#}"
                    );
                }
            }
        }
        tracing::warn!(
            handy_interaction_id = %handy_interaction_id,
            "handy 兜底轮询超过 24h 仍未应答，停止"
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_mapping_falls_back_to_confirm() {
        assert_eq!(map_interaction_kind("ask_user"), "ask_user");
        assert_eq!(map_interaction_kind("task_gate"), "task_gate");
        assert_eq!(map_interaction_kind("quant_confirm"), "confirm");
        assert_eq!(map_interaction_kind("team_gate"), "confirm");
        assert_eq!(map_interaction_kind(""), "confirm");
    }

    #[test]
    fn progress_percent_caps_at_90() {
        let mut p = Progress::new();
        assert_eq!(p.percent(), 10);
        for _ in 0..50 {
            p.record_tool("Bash");
        }
        assert_eq!(p.percent(), PROGRESS_CAP_RUNNING);
    }

    #[test]
    fn card_activities_keeps_last_8() {
        let mut p = Progress::new();
        for i in 0..12 {
            p.activities.push(format!("活动{i}"));
        }
        let acts = p.card_activities();
        assert_eq!(acts.len(), 8);
        assert_eq!(acts[0], "活动4");
        assert_eq!(acts[7], "活动11");
    }

    #[test]
    fn detail_defaults_and_uses_last_activity() {
        let mut p = Progress::new();
        assert_eq!(p.detail(), "正在执行");
        p.record_files(vec!["/tmp/a.rs".to_string()]);
        assert_eq!(p.detail(), "修改 /tmp/a.rs");
    }
}
