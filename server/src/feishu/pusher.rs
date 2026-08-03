//! 消费 run_chat_turn 的事件流，映射为飞书卡片刷新。
//!
//! 与 weixin/pusher.rs 的区别：微信是纯文本进度摘要（8s 节流），
//! 飞书用任务卡片原地刷新（2s 节流）+ AskUser 按钮确认卡片。

use crate::chat::EventEntry;
use crate::feishu::api::FeishuApi;
use crate::feishu::card::{
    build_confirm_card, build_multi_question_card, build_task_card, build_task_gate_card,
    build_task_title, ConfirmCardOptions, MultiQuestionCardOptions, TaskCardAction,
    TaskCardOptions, TaskGateCardOptions, TaskStatus, ThrottledCardUpdater, CARD_UPDATE_INTERVAL,
};
// 滞后/静默时回 EventBuffer 补齐事件的共享实现，微信 pusher 也用同一套。
use crate::task_state::{drain_buffer, next_event, Incoming, ProgressSnapshot};
use crate::AppState;
use anyhow::{bail, Context, Result};
use code_agent::AgentEvent;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::broadcast;

/// 最终答案文本截断上限（飞书单条文本消息不宜过长）。
/// 详情按钮分段发送也用同一上限。
pub(crate) const MAX_FINAL_TEXT_CHARS: usize = 8000;
/// 进度上限：任务未结束时最多到 90%，100% 只给真正完成
const PROGRESS_CAP_RUNNING: u32 = 90;

/// 起飞书进度 pusher。
///
/// `task_title` 是用户本轮原话 / 闸门 goal / 团队任务标题等任务摘要；
/// `existing_card_id` 为 router 已发的首响「已收到」卡 message_id，有则复用
/// 同一张卡原地更新到运行中/终态，无则自己 `reply_card` 新建。
/// 参数已多，加到 10 个会触发 too_many_arguments——这是渠道上下文的直传，
/// 拆结构体收益不大，故 allow。
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    state: Arc<AppState>,
    api: FeishuApi,
    message_id: String,
    chat_id: String,
    topic_id: String,
    session_id: String,
    task_title: String,
    existing_card_id: Option<String>,
    in_thread: bool,
    rx: broadcast::Receiver<EventEntry>,
) {
    tokio::spawn(run(
        state,
        api,
        message_id,
        chat_id,
        topic_id,
        session_id,
        task_title,
        existing_card_id,
        in_thread,
        rx,
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

/// 渠道上下文直传参数较多，见 `spawn` 注释。
#[allow(clippy::too_many_arguments)]
async fn run(
    state: Arc<AppState>,
    api: FeishuApi,
    message_id: String,
    chat_id: String,
    topic_id: String,
    session_id: String,
    task_title: String,
    existing_card_id: Option<String>,
    in_thread: bool,
    mut rx: broadcast::Receiver<EventEntry>,
) {
    // 标题算一次，四处 build_task_card 复用，避免各写一遍。
    let card_title = build_task_title(&task_title);

    let running_card = build_task_card(&TaskCardOptions {
        title: card_title.clone(),
        status: TaskStatus::Running,
        progress: 0,
        detail: "正在启动执行引擎".to_string(),
        activities: vec![],
        footer: None,
        actions: vec![],
        session_id: session_id.clone(),
        chat_id: chat_id.clone(),
        topic_id: topic_id.clone(),
    });

    // 复用 router 已发的首响卡（同一张卡从「已收到」原地更新到终态，
    // 不给用户多发消息）；没有则退化为自己新建。
    let card_id = match existing_card_id {
        Some(id) => {
            if let Err(e) = api.update_card(&id, &running_card).await {
                tracing::warn!("feishu: update ack card to running failed: {e:#}");
            }
            Some(id)
        }
        None => match api.reply_card(&message_id, &running_card, in_thread).await {
            Ok(id) => Some(id),
            Err(e) => {
                tracing::warn!("feishu: reply card failed, fallback to text: {e:#}");
                let _ = api.reply_text(&message_id, "已开始执行", in_thread).await;
                None
            }
        },
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
    // suggest_actions 可能在最终回复前若干轮就调用了，先存着，
    // 等 RunComplete 时和总结一起渲染进终态卡。
    let mut suggested: Vec<code_agent::SuggestedAction> = Vec::new();
    let mut llm_calls: u32 = 0;
    let mut input_tokens: u32 = 0;
    let mut output_tokens: u32 = 0;
    let started = Instant::now();

    let push_running = |progress: &Progress| {
        updater.push(build_task_card(&TaskCardOptions {
            title: card_title.clone(),
            status: TaskStatus::Running,
            progress: progress.percent(),
            detail: progress.detail(),
            activities: progress
                .activities
                .iter()
                .rev()
                .take(3)
                .rev()
                .cloned()
                .collect(),
            footer: None,
            // 运行中态不出现按钮，避免诱导用户在任务未完成时点击
            actions: vec![],
            session_id: session_id.clone(),
            chat_id: chat_id.clone(),
            topic_id: topic_id.clone(),
        }));
    };

    // 进度快照：让"进度怎样了"这类询问不必等卡片刷新就能读到当前状态。
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

    // 待处理事件队列：正常从 broadcast 收；滞后或长时间静默时从 EventBuffer 补齐。
    let mut queue: VecDeque<EventEntry> = VecDeque::new();
    let mut last_id: u64 = 0;
    // 终态事件已消费，收尾后跳出外层循环
    let mut finished = false;

    'outer: while !finished {
        if queue.is_empty() {
            match next_event(&mut rx).await {
                Incoming::Event(entry) => queue.push_back(entry),
                Incoming::Lagged(n) => {
                    tracing::warn!(
                        session_id = %session_id,
                        "feishu pusher lagged by {n} events, recovering from buffer"
                    );
                    let (missed, _) = drain_buffer(&state, &session_id, last_id).await;
                    queue.extend(missed);
                }
                Incoming::Idle | Incoming::Closed => {
                    let (missed, completed) = drain_buffer(&state, &session_id, last_id).await;
                    queue.extend(missed);
                    if queue.is_empty() && completed {
                        // 事件流已结束却没见到终态事件（滞后丢失或 agent 异常收尾）：
                        // 不能把卡片永远留在"运行中"，按未完成收尾。
                        tracing::warn!(
                            session_id = %session_id,
                            "feishu pusher: event stream ended without terminal event"
                        );
                        updater
                            .finish(build_task_card(&TaskCardOptions {
                                title: card_title.clone(),
                                status: TaskStatus::Failed,
                                progress: progress.percent(),
                                detail: "任务已结束，但未收到完成事件".to_string(),
                                activities: vec![progress.summary_line()]
                                    .into_iter()
                                    .filter(|s| !s.is_empty())
                                    .collect(),
                                footer: None,
                                actions: vec![],
                                session_id: session_id.clone(),
                                chat_id: chat_id.clone(),
                                topic_id: topic_id.clone(),
                            }))
                            .await;
                        let tail = if final_text.trim().is_empty() {
                            "任务已结束，但没有收到完成事件，请用 /status 确认结果".to_string()
                        } else {
                            let (body, _) = extract_file_markers(final_text.trim());
                            format!(
                                "{}\n\n（未收到完成事件，以上为已产出内容）",
                                truncate_final(&body)
                            )
                        };
                        let _ = api.reply_text(&message_id, &tail, in_thread).await;
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
                AgentEvent::RunStarted { .. } => {}
                AgentEvent::TextDelta { text } => {
                    final_text.push_str(&text);
                }
                AgentEvent::ToolStart { name, .. } => {
                    progress.record_tool(&name);
                    push_running(&progress);
                    publish(&progress).await;
                }
                AgentEvent::FileChanged { changes, .. } => {
                    progress.record_files(changes.into_iter().map(|c| c.path).collect());
                    push_running(&progress);
                    publish(&progress).await;
                }
                AgentEvent::PlanUpdated { .. } => {
                    progress.plan_count += 1;
                    progress.push_activity("更新执行计划".to_string());
                    push_running(&progress);
                    publish(&progress).await;
                }
                AgentEvent::AskUser {
                    question,
                    options,
                    kind,
                    questions,
                    ..
                } => {
                    // chat.rs forwarder / cli_agent 在 push 到 buffer 前已 await create_interaction，
                    // 此处 latest_pending 一定能读到刚落库的那一行（时序正确，无旁路 map）。
                    let interaction = state
                        .db
                        .latest_pending_interaction(&session_id)
                        .await
                        .ok()
                        .flatten();
                    let interaction_id = interaction
                        .as_ref()
                        .map(|r| r.id.clone())
                        .unwrap_or_default();
                    // interaction_id 为空时 admin_interaction_url 返回 None（与原先 filter 语义一致）
                    let admin_url =
                        crate::interaction_flow::admin_interaction_url(&state, &interaction_id);

                    // task_gate 单独开分支：大卡片结构与 quant_confirm / 普通 ask_user 不同，
                    // 强行复用 build_confirm_card 会弄坏 quant 确认路径。
                    let is_task_gate = kind.as_deref() == Some("task_gate")
                        || interaction.as_ref().is_some_and(|r| r.kind == "task_gate");
                    // 多问题：questions 非空走独立卡（逐题按钮 + 文字「1A 2B」提示）
                    let is_multi = !questions.is_empty();
                    let card = if is_task_gate {
                        let resume: Value = interaction
                            .as_ref()
                            .and_then(|r| r.resume_ref.as_deref())
                            .and_then(|raw| serde_json::from_str(raw).ok())
                            .unwrap_or_default();
                        // 缺字段 / null 表示查不到，保持 None（卡片不渲染改动提示）
                        let dirty_files = resume["dirty_files"].as_u64().map(|n| n as usize);
                        let backend = resume["backend"].as_str().unwrap_or("cli").to_string();
                        let goal = interaction
                            .as_ref()
                            .and_then(|r| r.goal.clone())
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| question.clone());
                        let analysis = interaction
                            .as_ref()
                            .and_then(|r| r.analysis.clone())
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| question.clone());
                        let chat = interaction
                            .as_ref()
                            .and_then(|r| r.chat_id.clone())
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| chat_id.clone());
                        let topic = interaction
                            .as_ref()
                            .and_then(|r| r.topic_id.clone())
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| topic_id.clone());
                        build_task_gate_card(&TaskGateCardOptions {
                            interaction_id: interaction_id.clone(),
                            session_id: session_id.clone(),
                            chat_id: chat,
                            topic_id: topic,
                            goal,
                            analysis,
                            backend,
                            source_label: "飞书派单".to_string(),
                            dirty_files,
                            admin_url,
                        })
                    } else if is_multi {
                        build_multi_question_card(&MultiQuestionCardOptions {
                            interaction_id: interaction_id.clone(),
                            session_id: session_id.clone(),
                            chat_id: chat_id.clone(),
                            topic_id: topic_id.clone(),
                            questions: questions.clone(),
                            answered: std::collections::HashMap::new(),
                            admin_url,
                        })
                    } else {
                        let is_quant = kind
                            .as_deref()
                            .is_some_and(|k| k.starts_with("quant_confirm:"));
                        let title = if is_quant {
                            "高成本操作确认"
                        } else {
                            "需要你的输入"
                        };
                        let hint = if is_quant {
                            Some(
                                "点「确认」执行本次；点「本会话全部同意」等同「确认50次」；\
                                 不同意可直接回复你的意见。也可文字回复「确认N次」（N≤50）"
                                    .to_string(),
                            )
                        } else {
                            Some("点击按钮或直接回复消息作答".to_string())
                        };
                        build_confirm_card(&ConfirmCardOptions {
                            title: title.to_string(),
                            question: question.clone(),
                            choices: options.clone(),
                            interaction_id: interaction_id.clone(),
                            session_id: session_id.clone(),
                            chat_id: chat_id.clone(),
                            topic_id: topic_id.clone(),
                            admin_url,
                            hint,
                        })
                    };
                    match api.reply_card(&message_id, &card, in_thread).await {
                        Ok(card_mid) => {
                            if !interaction_id.is_empty() {
                                if let Err(e) = state
                                    .db
                                    .set_interaction_card(&interaction_id, &card_mid)
                                    .await
                                {
                                    tracing::warn!(
                                        interaction_id = %interaction_id,
                                        "feishu: set_interaction_card failed: {e:#}"
                                    );
                                }
                            }
                            // 闸门卡 message_id 回填到 team_tasks.origin_message_id，
                            // 供后续主卡 reply 使用（建任务行时还没有卡片）。
                            // 失败只 warn，不影响卡片本身。
                            if let Some(team_task_id) = interaction
                                .as_ref()
                                .and_then(|r| r.resume_ref.as_deref())
                                .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                                .and_then(|v| {
                                    v.get("team_task_id")
                                        .and_then(|x| x.as_str())
                                        .filter(|s| !s.is_empty())
                                        .map(str::to_string)
                                })
                            {
                                if let Err(e) = state
                                    .db
                                    .set_team_task_origin_message(&team_task_id, &card_mid)
                                    .await
                                {
                                    tracing::warn!(
                                        %team_task_id,
                                        "feishu: set_team_task_origin_message failed: {e:#}"
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("feishu: send confirm card failed: {e:#}");
                            // 降级为纯文本提问
                            let mut msg = format!("❓ {question}");
                            if !options.is_empty() {
                                msg.push_str("\n选项：");
                                for (i, opt) in options.iter().enumerate() {
                                    msg.push_str(&format!("\n{}. {}", i + 1, opt));
                                }
                            }
                            if !interaction_id.is_empty() {
                                msg.push_str(&format!("\n任务编号：{interaction_id}"));
                            }
                            let _ = api.reply_text(&message_id, &msg, in_thread).await;
                        }
                    }
                    // ask_user / task_gate 期间 run 处于暂停：进度停在这里等人作答，
                    // 让"进度怎样了"能看出是在等人而不是在算。
                    progress.push_activity("等待用户确认".to_string());
                    publish(&progress).await;

                    // 闸门第一轮到此为止（cli_agent 紧接着发 TurnComplete，本 pusher 会退出）。
                    // 必须把进度卡 finish 掉：cancel() 只关闭 updater、不改卡片，
                    // 蓝色「运行中」会永久冻在 90% 以下，和闸门大卡并列烂尾。
                    // 第二轮由 resume_task_gate 另起一个 pusher 和一张新卡，不受影响。
                    if is_task_gate {
                        updater
                            .finish(build_task_card(&TaskCardOptions {
                                title: card_title.clone(),
                                status: TaskStatus::Waiting,
                                progress: progress.percent(),
                                detail: "已完成只读分析，等待你确认是否开始修".to_string(),
                                activities: vec![progress.summary_line()]
                                    .into_iter()
                                    .filter(|s| !s.is_empty())
                                    .collect(),
                                footer: None,
                                actions: vec![],
                                session_id: session_id.clone(),
                                chat_id: chat_id.clone(),
                                topic_id: topic_id.clone(),
                            }))
                            .await;
                    }
                }
                AgentEvent::SuggestedActions { actions } => {
                    // 同一轮多次调用以最后一次为准：模型改主意时不该叠加出 6 个按钮。
                    suggested = actions;
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
                    // 详情全文与建议动作进表而不进 callback value。
                    // 顺序即按钮顺序：建议动作在前（用户更可能点），查看详情兜底在最后。
                    // 写库失败不影响卡片主体，只是没有按钮——不能让它整轮失败。
                    let mut items: Vec<(String, String, String)> = suggested
                        .iter()
                        .map(|a| ("suggest".to_string(), a.label.clone(), a.prompt.clone()))
                        .collect();
                    items.push(("detail".to_string(), "查看详情".to_string(), body.clone()));
                    let terminal_actions = match state
                        .db
                        .create_feishu_card_actions(&session_id, &items)
                        .await
                    {
                        Ok(ids) => items
                            .iter()
                            .zip(ids.into_iter())
                            .map(|((kind, label, _), id)| TaskCardAction {
                                label: label.clone(),
                                action: if kind == "suggest" {
                                    "task_suggest".to_string()
                                } else {
                                    "task_detail".to_string()
                                },
                                action_id: id,
                            })
                            .collect(),
                        Err(e) => {
                            tracing::warn!(session_id, "写入卡片按钮 payload 失败: {e:#}");
                            vec![]
                        }
                    };
                    updater
                        .finish(build_task_card(&TaskCardOptions {
                            title: card_title.clone(),
                            status: TaskStatus::Success,
                            progress: 100,
                            detail: "执行完成".to_string(),
                            activities: vec![progress.summary_line()]
                                .into_iter()
                                .filter(|s| !s.is_empty())
                                .collect(),
                            footer: Some(footer.clone()),
                            actions: terminal_actions,
                            session_id: session_id.clone(),
                            chat_id: chat_id.clone(),
                            topic_id: topic_id.clone(),
                        }))
                        .await;
                    send_final_text(
                        &state,
                        &session_id,
                        &api,
                        &message_id,
                        &body,
                        &files,
                        &footer,
                        in_thread,
                    )
                    .await;
                    finished = true;
                    break;
                }
                AgentEvent::RunFailed { message, .. } => {
                    // 保留失败前的真实进度和工具摘要：写死 0% 会让"跑了很久才失败"
                    // 看起来像"根本没跑起来"，掩盖了实际执行到哪一步。
                    // 失败终态不加详情按钮（没有总结可展开）。
                    updater
                        .finish(build_task_card(&TaskCardOptions {
                            title: card_title.clone(),
                            status: TaskStatus::Failed,
                            progress: progress.percent(),
                            detail: message.clone(),
                            activities: vec![progress.summary_line()]
                                .into_iter()
                                .filter(|s| !s.is_empty())
                                .collect(),
                            footer: None,
                            actions: vec![],
                            session_id: session_id.clone(),
                            chat_id: chat_id.clone(),
                            topic_id: topic_id.clone(),
                        }))
                        .await;
                    let _ = api
                        .reply_text(&message_id, &format!("执行失败：{message}"), in_thread)
                        .await;
                    finished = true;
                    break;
                }
                AgentEvent::RunCancelled { .. } => {
                    updater
                        .finish(build_task_card(&TaskCardOptions {
                            title: card_title.clone(),
                            status: TaskStatus::Failed,
                            progress: progress.percent(),
                            detail: "任务已取消".to_string(),
                            activities: vec![],
                            footer: None,
                            actions: vec![],
                            session_id: session_id.clone(),
                            chat_id: chat_id.clone(),
                            topic_id: topic_id.clone(),
                        }))
                        .await;
                    let _ = api.reply_text(&message_id, "任务已取消", in_thread).await;
                    finished = true;
                    break;
                }
                AgentEvent::Error { message } => {
                    progress.push_activity(format!("出错：{message}"));
                    push_running(&progress);
                    publish(&progress).await;
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

    updater.cancel().await;
    state.tasks.clear_progress(&session_id).await;
}

/// 从最终文本中提取 [file:/路径] 标记（与 weixin/pusher.rs 同一约定）。
/// send_final_text 会校验路径和图片格式后上传飞书。
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

/// 正文截断到飞书单条文本可接受的长度，超长时提示去 web 端看全文。
fn truncate_final(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "已完成".to_string();
    }
    let mut t: String = trimmed.chars().take(MAX_FINAL_TEXT_CHARS).collect();
    if trimmed.chars().count() > MAX_FINAL_TEXT_CHARS {
        t.push_str("\n…（内容过长已截断，完整结果请到 web 端查看）");
    }
    t
}

async fn send_final_text(
    state: &Arc<AppState>,
    session_id: &str,
    api: &FeishuApi,
    message_id: &str,
    body: &str,
    files: &[String],
    footer: &str,
    in_thread: bool,
) {
    let mut msg = truncate_final(body);
    if !files.is_empty() {
        msg.push_str(&format!("\n\n图片将另行发送：{} 张", files.len()));
    }
    msg.push_str(&format!("\n{footer}"));
    if let Err(e) = api.reply_text(message_id, &msg, in_thread).await {
        tracing::warn!("feishu: send final text failed: {e:#}");
    }
    for path in files {
        match load_outbound_image(state, session_id, path).await {
            Ok(image) => {
                let sent = api
                    .reply_image(
                        message_id,
                        image.bytes,
                        &image.file_name,
                        image.media_type,
                        in_thread,
                    )
                    .await;
                if let Err(e) = sent {
                    tracing::warn!(path, "feishu: send image failed: {e:#}");
                    let _ = api
                        .reply_text(message_id, &format!("图片回传失败：{e:#}"), in_thread)
                        .await;
                } else if image.remove_after_send {
                    let _ = tokio::fs::remove_file(&image.canonical_path).await;
                }
            }
            Err(e) => {
                tracing::warn!(path, "feishu: rejected outbound file: {e:#}");
                let _ = api
                    .reply_text(message_id, &format!("图片回传被拒绝：{e:#}"), in_thread)
                    .await;
            }
        }
    }
}

struct OutboundImage {
    bytes: Vec<u8>,
    file_name: String,
    media_type: &'static str,
    canonical_path: std::path::PathBuf,
    remove_after_send: bool,
}

async fn load_outbound_image(
    state: &Arc<AppState>,
    session_id: &str,
    path: &str,
) -> Result<OutboundImage> {
    const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;
    let canonical_path = tokio::fs::canonicalize(path)
        .await
        .with_context(|| format!("图片不存在: {path}"))?;
    let temp_root = tokio::fs::canonicalize(std::env::temp_dir()).await?;
    let worktree = state
        .db
        .get_session(session_id)
        .await?
        .and_then(|session| session.work_dir);
    let worktree_root = match worktree {
        Some(path) => tokio::fs::canonicalize(path).await.ok(),
        None => None,
    };
    let in_temp = canonical_path.starts_with(&temp_root)
        && canonical_path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("trace-"));
    let in_worktree = worktree_root
        .as_ref()
        .is_some_and(|root| canonical_path.starts_with(root));
    if !in_temp && !in_worktree {
        bail!("文件不在当前 worktree 或 server 临时目录");
    }
    let metadata = tokio::fs::metadata(&canonical_path).await?;
    if !metadata.is_file() || metadata.len() > MAX_IMAGE_BYTES {
        bail!("文件不是普通图片或超过 10 MiB");
    }
    let bytes = tokio::fs::read(&canonical_path).await?;
    let media_type = image_media_type(&bytes).ok_or_else(|| anyhow::anyhow!("不支持的图片格式"))?;
    let file_name = canonical_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("trace-image")
        .to_string();
    Ok(OutboundImage {
        bytes,
        file_name,
        media_type,
        canonical_path,
        remove_after_send: in_temp,
    })
}

fn image_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
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

    #[test]
    fn detects_supported_image_types() {
        assert_eq!(
            image_media_type(b"\x89PNG\r\n\x1a\nrest"),
            Some("image/png")
        );
        assert_eq!(image_media_type(b"not-an-image"), None);
    }
}
