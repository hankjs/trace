//! 团队任务编排器：把 decide_next 的判定变成真实的派发 / 开闸门 / 收尾。
//!
//! 单一入口 `advance`。所有状态推进都必须从这里走，不要在别处直接改
//! team_tasks.status——状态机分支多，两个写入点必然漂移。
//!
//! 幂等性由三重防线保证：
//! 1. `decide_next` 对重复触发与终态任务返回 `Decision::Ignore`
//! 2. `team_task_runs` 的 (task_id, role, round) 唯一键，重复派发插入失败
//! 3. `TaskRegistry` 名额，同一 session 同时只有一个 run

use super::roles::{RolePromptInput, UpstreamHandoff};
use super::{
    decide_next, parse_handoff, role_def, role_prompt, DecideInput, Decision, GateBoundary, Handoff,
    RunOutcome, Trigger as DecideTrigger, Verdict,
};
use crate::cli_agent;
use crate::AppState;
use anyhow::{anyhow, Context, Result};
use code_agent::AgentEvent;
use hank_db::{NewInteraction, TeamTask, TeamTaskRun};
use hank_provider::ContentBlock;
use std::sync::{Arc, OnceLock};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// run 终态回调解耦
// ---------------------------------------------------------------------------
//
// cli_agent::execute_remote_turn 若直接 await advance，会形成 future 类型环：
//   execute_remote_turn → advance → dispatch_role → run_cli_turn → execute_remote_turn
// rustc 推 Send 时会 E0391 循环依赖或 ICE。
// 用无界 channel 把「通知」和「推进」拆开：cli_agent 只 send，worker 里再 advance。

struct RunFinishedMsg {
    state: Arc<AppState>,
    session_id: String,
    succeeded: bool,
    final_text: Option<String>,
}

static RUN_FINISHED_TX: OnceLock<mpsc::UnboundedSender<RunFinishedMsg>> = OnceLock::new();

/// 启动 run 终态消费协程。在 main 里调一次即可（与 team_task.enabled 无关——
/// 关闭时 notify 侧会早退，worker 空转无成本）。
pub fn start_run_finished_worker() {
    let (tx, mut rx) = mpsc::unbounded_channel::<RunFinishedMsg>();
    if RUN_FINISHED_TX.set(tx).is_err() {
        return; // 已启动
    }
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            handle_run_finished_msg(msg).await;
        }
    });
}

/// 供 cli_agent 在 run 终态调用。只入队，不 await advance（打断类型环）。
pub fn enqueue_run_finished(
    state: Arc<AppState>,
    session_id: String,
    succeeded: bool,
    final_text: Option<String>,
) {
    let Some(tx) = RUN_FINISHED_TX.get() else {
        tracing::warn!(
            session_id = %session_id,
            "team_task run_finished worker 未启动，丢弃终态回调"
        );
        return;
    };
    if tx
        .send(RunFinishedMsg {
            state,
            session_id,
            succeeded,
            final_text,
        })
        .is_err()
    {
        tracing::warn!("team_task run_finished channel 已关闭");
    }
}

async fn handle_run_finished_msg(msg: RunFinishedMsg) {
    let RunFinishedMsg {
        state,
        session_id,
        succeeded,
        final_text,
    } = msg;
    let settings = super::settings::effective(&state).await;
    if !settings.enabled {
        return;
    }
    let Ok(Some(session)) = state.db.get_session(&session_id).await else {
        return;
    };
    let metadata = parse_session_metadata(session.metadata.as_deref());
    let Some(task_id) = metadata["team_task_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
    else {
        return;
    };

    let task = match state.db.get_team_task(&task_id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            tracing::warn!(%task_id, session_id = %session_id, "run_finished: 任务不存在");
            return;
        }
        Err(e) => {
            tracing::warn!(%task_id, "run_finished: 读任务失败: {e:#}");
            return;
        }
    };

    // 闸门态不应由 run 终态推进（gate_mode 成功路径不应入队；这里再挡一层）。
    if !super::is_running(&task.status) {
        tracing::debug!(
            %task_id,
            status = %task.status,
            "run_finished: 任务不在 running_*，跳过"
        );
        return;
    }

    let Some(role) = task.current_role.clone() else {
        tracing::warn!(%task_id, "run_finished: current_role 为空，跳过");
        return;
    };

    let round = match state.db.latest_team_run(&task_id).await {
        Ok(Some(run)) => run.round,
        Ok(None) => 1,
        Err(e) => {
            tracing::warn!(%task_id, "run_finished: 读 latest run 失败: {e:#}");
            1
        }
    };

    let outcome = if succeeded {
        RunOutcome::Finished(Verdict::Pass)
    } else {
        RunOutcome::Failed
    };

    // 回调失败只 warn：run 已跑完，编排失败不该让用户看到「任务失败」。
    if let Err(e) = advance(
        &state,
        &task_id,
        Trigger::RunFinished {
            role,
            round,
            outcome,
            final_text,
        },
    )
    .await
    {
        tracing::warn!(
            %task_id,
            session_id = %session_id,
            "run_finished: advance 失败（run 已结束，不影响用户侧终态）: {e:#}"
        );
    }
}

// ---------------------------------------------------------------------------
// 编排器入口类型（owned，便于跨 await）
// ---------------------------------------------------------------------------

/// 编排器入口的触发源。持有 owned String，便于跨 await 传递；
/// 内部转换成 mod.rs 的借用版 Trigger<'_> 交给 decide_next。
#[derive(Debug, Clone)]
pub enum Trigger {
    /// 闸门被应答（飞书按钮 / admin 手动应答共用）
    GateAnswered {
        /// 交互单 id，事件审计用；状态机本身不读
        #[allow(dead_code)]
        interaction_id: String,
        answer: String,
    },
    /// 某角色 run 走到终态。
    ///
    /// `final_text` 是角色输出正文，用于 parse_handoff。
    RunFinished {
        role: String,
        round: i32,
        outcome: RunOutcome,
        final_text: Option<String>,
    },
    /// 看板 / 飞书 /stop 取消。
    Cancelled { operator: String },
}

// ---------------------------------------------------------------------------
// advance — 唯一入口
// ---------------------------------------------------------------------------

/// 推进任务到下一步。
///
/// **这是状态推进的唯一入口**：所有写 `team_tasks.status` 的路径都必须从这里走。
/// 状态机分支多，如果另开写入点，两处必然漂移（见设计文档 §6.4）。
///
/// 幂等：重复调用同一状态不会重复派发。返回 Err 只表示「推进过程本身出错」
/// （读不到任务、DB 挂了），任务被判为 Ignore 不是错误。
pub async fn advance(state: &Arc<AppState>, task_id: &str, trigger: Trigger) -> Result<()> {
    // 1. 读任务
    let task = state
        .db
        .get_team_task(task_id)
        .await
        .context("读团队任务")?
        .ok_or_else(|| anyhow!("团队任务不存在: {task_id}"))?;

    // 2. RunFinished：先收尾 run 行，拿到归一后的 verdict 供 decide_next 使用
    let run_finished_outcome: Option<(String, i32, RunOutcome)> = match &trigger {
        Trigger::RunFinished {
            role,
            round,
            outcome,
            final_text,
        } => {
            let verdict = finalize_run(
                state,
                &task,
                role,
                *round,
                outcome,
                final_text.as_deref(),
            )
            .await
            .context("收尾 team_task_run")?;
            let outcome_for_decide = match outcome {
                RunOutcome::Failed => RunOutcome::Failed,
                RunOutcome::Finished(_) => RunOutcome::Finished(verdict),
            };
            Some((role.clone(), *round, outcome_for_decide))
        }
        _ => None,
    };

    // 运行时配置：DB 优先（admin 可改、即时生效）
    let settings = super::settings::effective(state).await;

    // 3. 状态机判定。decide_next 的输入全是借用，必须在本块内算完 decision，
    // 不要把带 lifetime 的 DecideTrigger 跨后续 await 持有——否则 future 状态机
    // 会把借用塞进跨 await 的态，排查 Send 问题时极难定位。
    let decision = {
        let decide_trigger = match &trigger {
            Trigger::GateAnswered { answer, .. } => DecideTrigger::GateAnswered {
                answer: answer.as_str(),
            },
            Trigger::RunFinished { .. } => {
                let (role, round, outcome) = run_finished_outcome
                    .as_ref()
                    .expect("RunFinished 必有 outcome");
                DecideTrigger::RunFinished {
                    role: role.as_str(),
                    round: *round,
                    outcome: *outcome,
                }
            }
            Trigger::Cancelled { operator } => DecideTrigger::Cancelled {
                operator: operator.as_str(),
            },
        };
        let input = DecideInput {
            status: &task.status,
            current_role: task.current_role.as_deref(),
            dev_rounds: task.dev_rounds,
            trigger: decide_trigger,
        };
        decide_next(&input, &settings)
    };

    // 4. 分派（此后不再借用 trigger 的短生命周期字段）
    let cancel_operator = match &trigger {
        Trigger::Cancelled { operator } => Some(operator.clone()),
        _ => None,
    };

    match decision {
        Decision::Ignore { reason } => {
            tracing::info!(task_id, %reason, "team_task advance: Ignore");
            Ok(())
        }
        Decision::OpenGate { boundary } => {
            open_gate(state, &task, boundary).await?;
            let _ = append_event(
                state,
                task_id,
                "gate_opened",
                task.current_role.as_deref(),
                None,
                None,
                Some(&format!("boundary={}", boundary.as_str())),
            )
            .await;
            // 主卡刷新是可观测性：best-effort，不接收返回值
            super::card::sync_team_card(state, task_id).await;
            Ok(())
        }
        Decision::DispatchRole { role, round } => {
            let started = dispatch_role(state, &task, &role, round).await?;
            if started {
                let _ = append_event(
                    state,
                    task_id,
                    "role_started",
                    Some(&role),
                    Some(round),
                    None,
                    None,
                )
                .await;
            }
            // dispatch_role 成功路径内部已 sync（先建主卡再起 pusher）；
            // 跳过路径也刷一次，让流转记录及时出现。
            if !started {
                super::card::sync_team_card(state, task_id).await;
            }
            Ok(())
        }
        Decision::Finish { status, reason } => {
            finish_task(state, &task, status, reason.as_deref()).await?;
            let _ = append_event(
                state,
                task_id,
                "status_changed",
                None,
                None,
                cancel_operator.as_deref(),
                reason.as_deref(),
            )
            .await;
            super::card::sync_team_card(state, task_id).await;
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// RunFinished 收尾
// ---------------------------------------------------------------------------

/// 收尾某角色的 run 行，返回归一后的 verdict 供 decide_next 使用。
///
/// verdict 归一规则（重要）：
/// - handoff.verdict == None（模型没写 verdict 行）→ Unknown
/// - handoff.verdict == Some(Unknown)（写了但解析不出来）→ Unknown
/// - 不需要 verdict 的角色（开发）→ 强制 Pass，忽略 handoff 里的 verdict
///
/// 前两种都归到 Unknown，按状态机规则任务会 failed。这是刻意的：
/// 见设计文档 §6.3——猜 pass 会让评审形同虚设，猜 reject 会无谓返工。
async fn finalize_run(
    state: &Arc<AppState>,
    task: &TeamTask,
    role: &str,
    round: i32,
    outcome: &RunOutcome,
    final_text: Option<&str>,
) -> Result<Verdict> {
    let needs_verdict = role_def(role).map(|d| d.needs_verdict).unwrap_or(true);
    let handoff = final_text.map(parse_handoff).unwrap_or_default();
    let verdict = match outcome {
        RunOutcome::Failed => {
            // 失败路径不看 handoff verdict；仍用 normalize 以便 needs_verdict=false 时落 Pass
            // 实际 decide_next 看的是 Failed，不读这里的返回值语义
            normalize_verdict(handoff.verdict, needs_verdict)
        }
        RunOutcome::Finished(_) => normalize_verdict(handoff.verdict, needs_verdict),
    };

    let runs = state.db.list_team_runs(&task.id).await.unwrap_or_default();
    let run_row = runs.iter().find(|r| r.role == role && r.round == round);

    let Some(run_row) = run_row else {
        tracing::warn!(
            task_id = %task.id,
            role,
            round,
            "finalize_run: 找不到对应 run 行，跳过落库"
        );
        return Ok(verdict);
    };

    let status = match outcome {
        RunOutcome::Failed => "failed",
        RunOutcome::Finished(_) => "finished",
    };
    // 开发角色 changed_files 为 None 时不在此补 git diff——那要远程 shell，
    // 属第 5 步接入时的事；这里保持 None 落库。
    let dirty_files = handoff.changed_files;
    let handoff_json = handoff_to_json(&handoff);
    let summary = handoff.summary.as_deref();
    let verdict_str = match outcome {
        RunOutcome::Failed => Some("failed"),
        RunOutcome::Finished(_) => Some(verdict.as_str()),
    };
    let error = match outcome {
        RunOutcome::Failed => Some("角色执行失败"),
        RunOutcome::Finished(_) => None,
    };

    if let Err(e) = state
        .db
        .finish_team_run(
            &run_row.id,
            status,
            verdict_str,
            Some(&handoff_json),
            summary,
            dirty_files,
            error,
        )
        .await
    {
        tracing::warn!(
            run_id = %run_row.id,
            task_id = %task.id,
            "finish_team_run 失败: {e:#}"
        );
    }

    // 流转记录及时出现；best-effort，不接收返回值
    super::card::sync_team_card(state, &task.id).await;

    Ok(verdict)
}

/// 把 parse_handoff 的结果归一成 decide_next 要的 Verdict。
/// 抽成纯函数是为了能单测——这段规则错了会让评审形同虚设。
fn normalize_verdict(handoff_verdict: Option<Verdict>, needs_verdict: bool) -> Verdict {
    if !needs_verdict {
        // 开发角色不要求 verdict，强制 Pass（忽略 handoff 里写了什么）
        return Verdict::Pass;
    }
    match handoff_verdict {
        None => Verdict::Unknown,
        Some(v) => v,
    }
}

fn handoff_to_json(h: &Handoff) -> String {
    serde_json::json!({
        "verdict": h.verdict.map(|v| v.as_str()),
        "changed_files": h.changed_files,
        "summary": h.summary,
        "blocking": h.blocking,
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// OpenGate
// ---------------------------------------------------------------------------

/// 开人工闸门：落 team_gate 交互单 + emit AskUser 让 pusher 出卡片，
/// 任务状态改 pending_*_gate。
async fn open_gate(
    state: &Arc<AppState>,
    task: &TeamTask,
    boundary: GateBoundary,
) -> Result<()> {
    let options = gate_options(boundary);
    let options_json =
        serde_json::to_string(&options).unwrap_or_else(|_| r#"["继续","终止"]"#.to_string());
    let resume_ref = serde_json::json!({
        "team_task_id": task.id,
        "boundary": boundary.as_str(),
        "round": task.dev_rounds,
    })
    .to_string();

    let title = gate_title(boundary);
    let goal = task.goal.as_deref().unwrap_or(&task.title);
    let channel = if task.source.is_empty() {
        "feishu"
    } else {
        task.source.as_str()
    };

    let row = state
        .db
        .create_interaction(NewInteraction {
            session_id: &task.session_id,
            user_id: &task.user_id,
            channel,
            account_id: task.account_id.as_deref(),
            chat_id: task.chat_id.as_deref(),
            topic_id: task.topic_id.as_deref(),
            kind: "team_gate",
            title,
            goal: Some(goal),
            analysis: task.analysis.as_deref(),
            options: &options_json,
            resume_ref: Some(&resume_ref),
            // 飞书渠道不过期，与现有 task_gate 一致
            expires_at: None,
        })
        .await
        .context("创建 team_gate 交互单")?;

    // current_role 保持不变，只改 status。
    // decide_next 在 pending_*_gate 分支要靠 current_role 算下一个角色
    // （next_role(cfg.roles, current)）；清掉之后 dispatch_from_pending_gate
    // 的兜底会静默派发错角色（设计文档 §6.4）。
    state
        .db
        .update_team_task_status(
            &task.id,
            boundary.pending_status(),
            task.current_role.as_deref(),
            None,
            None,
        )
        .await
        .context("更新任务为 pending 闸门状态")?;

    emit_ask_user(
        state,
        &task.session_id,
        AgentEvent::AskUser {
            question: goal.to_string(),
            options,
            tool_use_id: format!("team_gate:{}", row.id),
            kind: Some("team_gate".to_string()),
            questions: Vec::new(),
        },
    )
    .await;

    tracing::info!(
        task_id = %task.id,
        interaction_id = %row.id,
        boundary = boundary.as_str(),
        "team_gate 交互单已落表"
    );
    Ok(())
}

/// 闸门按钮文案。抽成纯函数便于单测四个边界。
fn gate_options(boundary: GateBoundary) -> Vec<String> {
    match boundary {
        GateBoundary::DevStart => vec!["开始修".into(), "跳过".into()],
        GateBoundary::ReviewStart => vec!["继续评审".into(), "终止".into()],
        GateBoundary::DevRestart => vec!["重新开发".into(), "终止".into()],
        GateBoundary::TestStart => vec!["继续测试".into(), "终止".into()],
    }
}

fn gate_title(boundary: GateBoundary) -> &'static str {
    match boundary {
        GateBoundary::DevStart => "新任务 · 待确认是否开始修",
        GateBoundary::ReviewStart => "开发完成 · 是否进入评审",
        GateBoundary::DevRestart => "评审打回 · 是否重新开发",
        GateBoundary::TestStart => "评审通过 · 是否进入测试",
    }
}

// ---------------------------------------------------------------------------
// DispatchRole
// ---------------------------------------------------------------------------

/// 派发某角色的某一轮。
///
/// 顺序不能重排：抢名额 → 插 run 行（唯一键防重）→ 写 thread → 派发。
/// 先抢名额是因为 active_tasks 要等 run_cli_turn 走完准备工作才登记，
/// 中间有秒级空窗（见 task_state.rs 模块注释）。
///
/// 返回 `Ok(true)` 表示已成功派发；`Ok(false)` 表示幂等跳过（已有在途 /
/// 唯一键冲突）；`Err` 表示派发过程出错且已回滚为 failed。
async fn dispatch_role(
    state: &Arc<AppState>,
    task: &TeamTask,
    role: &str,
    round: i32,
) -> Result<bool> {
    let role_def = role_def(role).ok_or_else(|| anyhow!("未知角色: {role}"))?;
    let running_status = role_def.running_status;
    let session_id = task.session_id.clone();
    let task_id = task.id.clone();
    let backend = task.backend.clone();

    // 1. 抢名额——必须在插 run 行与改状态之前。
    // active_tasks 要等 run_cli_turn 走完鉴权/准备工作才登记，中间有秒级空窗。
    let dispatch_guard = match state.tasks.try_acquire(&session_id).await {
        Some(guard) => {
            if state.active_tasks.read().await.contains_key(&session_id) {
                guard.release().await;
                tracing::info!(
                    task_id = %task_id,
                    session_id = %session_id,
                    "已有在途派发（active_tasks），跳过"
                );
                let _ = append_event(
                    state,
                    &task_id,
                    "role_started",
                    Some(role),
                    Some(round),
                    None,
                    Some("已有在途派发，跳过"),
                )
                .await;
                return Ok(false);
            }
            guard
        }
        None => {
            tracing::info!(
                task_id = %task_id,
                session_id = %session_id,
                "已有在途派发（TaskRegistry），跳过"
            );
            let _ = append_event(
                state,
                &task_id,
                "role_started",
                Some(role),
                Some(round),
                None,
                Some("已有在途派发，跳过"),
            )
            .await;
            return Ok(false);
        }
    };

    // 2. 插 run 行；唯一键冲突 = 已派发过
    let run_row = match state
        .db
        .insert_team_run(&task_id, role, round)
        .await
        .context("插入 team_task_run")?
    {
        Some(row) => row,
        None => {
            dispatch_guard.release().await;
            tracing::info!(
                task_id = %task_id,
                role,
                round,
                "run 行唯一键冲突，跳过重复派发"
            );
            return Ok(false);
        }
    };
    let run_id = run_row.id.clone();

    if role == "developer" {
        if let Err(e) = state.db.bump_team_task_dev_rounds(&task_id).await {
            tracing::warn!(task_id = %task_id, "bump dev_rounds 失败: {e:#}");
        }
    }

    // 3. 写 thread + team_task_id
    if let Err(e) = prepare_session_for_role(state, &session_id, &task_id).await {
        let err_msg = format!("准备 session metadata 失败: {e:#}");
        let _ = state
            .db
            .finish_team_run(&run_id, "failed", None, None, None, None, Some(&err_msg))
            .await;
        let _ = state
            .db
            .update_team_task_status(&task_id, "failed", None, None, Some(&err_msg))
            .await;
        state.tasks.clear_progress(&session_id).await;
        dispatch_guard.release().await;
        let _ = append_event(
            state,
            &task_id,
            "status_changed",
            Some(role),
            Some(round),
            None,
            Some(&err_msg),
        )
        .await;
        return Err(e);
    }

    // 4. 构造 prompt（owned，借用不跨 await）
    let settings = super::settings::effective(state).await;
    let prompt_text = {
        let runs = state.db.list_team_runs(&task_id).await.unwrap_or_default();
        let upstream_run = pick_upstream_run(&runs, role, &settings.roles);
        let (blocking_owned, verdict_owned) = upstream_extras(upstream_run);
        let changed_files = upstream_run.and_then(|r| {
            r.dirty_files.or_else(|| {
                r.handoff
                    .as_ref()
                    .and_then(|h| serde_json::from_str::<serde_json::Value>(h).ok())
                    .and_then(|v| {
                        v.get("changed_files")
                            .and_then(|c| c.as_i64().map(|n| n as i32))
                    })
            })
        });
        let upstream = upstream_run.map(|r| UpstreamHandoff {
            role: &r.role,
            summary: r.summary.as_deref(),
            verdict: verdict_owned,
            blocking: blocking_owned.as_deref(),
            changed_files,
        });
        let goal = task.goal.as_deref().unwrap_or(&task.title);
        let prompt_input = RolePromptInput {
            goal,
            analysis: task.analysis.as_deref(),
            agent_kind: &task.agent_kind,
            round,
            upstream,
        };
        role_prompt(role, &prompt_input).ok_or_else(|| anyhow!("无法构造角色 prompt: {role}"))?
    };

    // 5. 改任务状态
    state
        .db
        .update_team_task_status(&task_id, running_status, Some(role), None, None)
        .await
        .context("更新任务为 running 状态")?;

    // 6. 派发
    let session = state
        .db
        .get_session(&session_id)
        .await
        .context("读 session")?
        .ok_or_else(|| anyhow!("会话不存在: {session_id}"))?;
    let content = vec![ContentBlock::Text { text: prompt_text }];
    let turn = cli_agent::run_cli_turn(state, &session_id, Some(session), content, &backend).await;

    // 7. 失败回滚——不做自动重试（工作区可能已改了一半）。
    match turn {
        Ok(handle) => {
            // 先建/刷主卡、再起 pusher——否则首个角色的进度仍然丢
            // （spawn_role_pusher 依赖 card_message_id，此前从未写入）。
            super::card::sync_team_card(state, &task_id).await;
            // 重读拿到刚写入的 card_message_id；失败则退回原 task 快照
            let task_for_pusher = match state.db.get_team_task(&task_id).await {
                Ok(Some(t)) => t,
                _ => {
                    // 状态已改 running，用更新后的字段拼一份
                    let mut t = task.clone();
                    t.status = running_status.to_string();
                    t.current_role = Some(role.to_string());
                    t
                }
            };
            // 进度卡是附加功能：缺主卡/账号只 warn。独立 spawn 避免并进 advance future。
            let state_p = state.clone();
            let rx = handle.event_rx;
            tokio::spawn(async move {
                spawn_role_pusher_if_possible(&state_p, &task_for_pusher, rx).await;
            });
            dispatch_guard.release().await;
            Ok(true)
        }
        Err(e) => {
            let err_msg = format!("{e:#}");
            let _ = state
                .db
                .finish_team_run(&run_id, "failed", None, None, None, None, Some(&err_msg))
                .await;
            let _ = state
                .db
                .update_team_task_status(&task_id, "failed", None, None, Some(&err_msg))
                .await;
            state.tasks.clear_progress(&session_id).await;
            dispatch_guard.release().await;
            let _ = append_event(
                state,
                &task_id,
                "status_changed",
                Some(role),
                Some(round),
                None,
                Some(&err_msg),
            )
            .await;
            Err(e).context("派发角色 run 失败")
        }
    }
}


/// 起飞书进度 pusher。进度卡是附加功能：没有主卡 / 账号停用 / 查不到时只 warn，
/// 不阻断任务执行。调用方须先 `sync_team_card` 再调本函数，否则
/// `card_message_id` 仍为空会跳过首个角色的进度。
async fn spawn_role_pusher_if_possible(
    state: &Arc<AppState>,
    task: &TeamTask,
    event_rx: tokio::sync::broadcast::Receiver<crate::chat::EventEntry>,
) {
    let Some(card_message_id) = task
        .card_message_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
    else {
        tracing::warn!(
            task_id = %task.id,
            "team_task 尚无 card_message_id，跳过 pusher（请确认 sync_team_card 已先执行且 origin_message_id 有值）"
        );
        return;
    };
    let Some(account_id) = task.account_id.as_deref().filter(|s| !s.is_empty()) else {
        tracing::warn!(task_id = %task.id, "team_task 无 account_id，跳过 pusher");
        return;
    };
    let Some(chat_id) = task.chat_id.clone().filter(|s| !s.is_empty()) else {
        tracing::warn!(task_id = %task.id, "team_task 无 chat_id，跳过 pusher");
        return;
    };
    let topic_id = task
        .topic_id
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "main".to_string());

    let account = match state.db.get_feishu_account(account_id).await {
        Ok(Some(a)) if a.enabled => a,
        Ok(Some(_)) => {
            tracing::warn!(task_id = %task.id, account_id, "飞书账号已停用，跳过 pusher");
            return;
        }
        Ok(None) => {
            tracing::warn!(task_id = %task.id, account_id, "飞书账号不存在，跳过 pusher");
            return;
        }
        Err(e) => {
            tracing::warn!(task_id = %task.id, account_id, "读飞书账号失败，跳过 pusher: {e:#}");
            return;
        }
    };

    let api = crate::feishu::api::FeishuApi::new_archived(&account, state.db.clone());
    let in_thread = topic_id != "main";
    crate::feishu::pusher::spawn(
        state.clone(),
        api,
        card_message_id,
        chat_id,
        topic_id,
        task.session_id.clone(),
        task.title.clone(),
        in_thread,
        event_rx,
    );
}

/// 从已完成的 run 列表里挑出给本角色做输入的那一轮（最近一个已完成的上游角色）。
///
/// - 非首角色：取配置顺序中上一个角色的最近 finished
/// - 首角色（开发）：取配置中下一个角色的最近 finished（打回场景；首轮没有则为 None）
fn pick_upstream_run<'a>(
    runs: &'a [TeamTaskRun],
    for_role: &str,
    cfg_roles: &[String],
) -> Option<&'a TeamTaskRun> {
    let idx = cfg_roles.iter().position(|r| r == for_role);
    let upstream_role: &str = match idx {
        Some(0) => cfg_roles.get(1).map(|s| s.as_str())?,
        Some(i) => cfg_roles.get(i - 1).map(|s| s.as_str())?,
        None => return None,
    };
    runs.iter()
        .rfind(|r| r.status == "finished" && r.role == upstream_role)
}

/// 从 run 的 handoff JSON / verdict 列解析 UpstreamHandoff 需要的 owned 字段。
fn upstream_extras(run: Option<&TeamTaskRun>) -> (Option<String>, Option<Verdict>) {
    let Some(run) = run else {
        return (None, None);
    };
    let verdict = run
        .verdict
        .as_deref()
        .map(Verdict::parse)
        .filter(|v| !matches!(v, Verdict::Unknown))
        .or_else(|| {
            run.handoff
                .as_ref()
                .and_then(|h| serde_json::from_str::<serde_json::Value>(h).ok())
                .and_then(|v| {
                    v.get("verdict")
                        .and_then(|x| x.as_str())
                        .map(Verdict::parse)
                })
        });
    let blocking = run.handoff.as_ref().and_then(|h| {
        serde_json::from_str::<serde_json::Value>(h)
            .ok()
            .and_then(|v| {
                v.get("blocking")
                    .and_then(|b| b.as_str())
                    .filter(|s| {
                        let t = s.trim();
                        !t.is_empty() && !t.eq_ignore_ascii_case("none") && t != "无"
                    })
                    .map(|s| s.to_string())
            })
    });
    (blocking, verdict)
}

/// 写 sessions.metadata：设 team_task_id，并把 agent_thread_id 置 null。
/// 不调用 cli_agent 的私有 parse_metadata / persist_thread_id。
async fn prepare_session_for_role(
    state: &Arc<AppState>,
    session_id: &str,
    team_task_id: &str,
) -> Result<()> {
    let session = state
        .db
        .get_session(session_id)
        .await?
        .ok_or_else(|| anyhow!("会话不存在: {session_id}"))?;
    let mut metadata = parse_session_metadata(session.metadata.as_deref());
    metadata["team_task_id"] = serde_json::Value::String(team_task_id.to_string());
    // 新角色新轮次 → null，让 CLI 开新 thread
    metadata["agent_thread_id"] = serde_json::Value::Null;
    state
        .db
        .update_session_metadata(session_id, &metadata.to_string())
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 看板重试
// ---------------------------------------------------------------------------

/// 从失败时的角色重开一轮（round + 1）。
///
/// 为什么用 `round + 1` 而不是复用原 round：`team_task_runs` 的
/// `(task_id, role, round)` 唯一键会拒绝重复插入。递增轮次同时保留了
/// 失败那轮的记录，看板上能看到「第 1 轮失败、第 2 轮重试」。
///
/// 返回 `Ok(true)` 已派发；`Ok(false)` 并发跳过（已有在途）；
/// `Err` 前置条件不满足或派发失败。
pub async fn retry_from_current_role(state: &Arc<AppState>, task_id: &str) -> Result<bool> {
    let task = state
        .db
        .get_team_task(task_id)
        .await
        .context("读团队任务")?
        .ok_or_else(|| anyhow!("团队任务不存在: {task_id}"))?;

    if task.status != super::STATUS_FAILED {
        return Err(anyhow!(
            "仅 failed 状态可重试，当前 status={}",
            task.status
        ));
    }

    let latest = state
        .db
        .latest_team_run(task_id)
        .await
        .context("读 latest run")?
        .ok_or_else(|| anyhow!("没有可重试的角色轮次"))?;

    let role = latest.role.clone();
    let new_round = latest.round + 1;
    let role_def = role_def(&role).ok_or_else(|| anyhow!("未知角色: {role}"))?;

    // 先把状态改回 running_*，再派发；dispatch_role 内部会再写一次
    state
        .db
        .update_team_task_status(
            task_id,
            role_def.running_status,
            Some(&role),
            None,
            None,
        )
        .await
        .context("重试前更新任务状态")?;

    // 重读，让 dispatch 用最新状态
    let task = state
        .db
        .get_team_task(task_id)
        .await
        .context("重读团队任务")?
        .ok_or_else(|| anyhow!("团队任务不存在: {task_id}"))?;

    let started = dispatch_role(state, &task, &role, new_round).await?;
    if started {
        let _ = append_event(
            state,
            task_id,
            "role_started",
            Some(&role),
            Some(new_round),
            None,
            Some("看板重试"),
        )
        .await;
    } else {
        // 并发跳过：状态已被改成 running_*，但没真正派发。
        // 把状态拨回 failed，避免看板显示「运行中」却无 run。
        let _ = state
            .db
            .update_team_task_status(
                task_id,
                super::STATUS_FAILED,
                None,
                None,
                Some("重试时已有在途派发，请稍后重试"),
            )
            .await;
    }
    Ok(started)
}

// ---------------------------------------------------------------------------
// Finish
// ---------------------------------------------------------------------------

/// 走终态：写状态与 finished_at、清 current_role、清进度快照。
async fn finish_task(
    state: &Arc<AppState>,
    task: &TeamTask,
    status: &str,
    reason: Option<&str>,
) -> Result<()> {
    // 终态才清 current_role（传 None）
    state
        .db
        .update_team_task_status(&task.id, status, None, None, reason)
        .await
        .context("更新任务终态")?;

    state.tasks.clear_progress(&task.session_id).await;

    // 清掉 sessions.metadata.team_task_id，否则该 session 后续的普通任务会被
    // should_gate_turn 的 in_team_pipeline 短路误判成「在流水线里」而不弹闸门。
    if let Err(e) = clear_session_team_task_id(state, &task.session_id).await {
        tracing::warn!(
            session_id = %task.session_id,
            task_id = %task.id,
            "清除 session.team_task_id 失败: {e:#}"
        );
    }

    tracing::info!(
        task_id = %task.id,
        status,
        ?reason,
        "团队任务终态"
    );
    Ok(())
}

async fn clear_session_team_task_id(state: &Arc<AppState>, session_id: &str) -> Result<()> {
    let session = state
        .db
        .get_session(session_id)
        .await?
        .ok_or_else(|| anyhow!("会话不存在: {session_id}"))?;
    let mut metadata = parse_session_metadata(session.metadata.as_deref());
    if let Some(obj) = metadata.as_object_mut() {
        obj.remove("team_task_id");
    }
    state
        .db
        .update_session_metadata(session_id, &metadata.to_string())
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 辅助
// ---------------------------------------------------------------------------

fn parse_session_metadata(raw: Option<&str>) -> serde_json::Value {
    raw.and_then(|v| serde_json::from_str(v).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

async fn emit_ask_user(state: &Arc<AppState>, session_id: &str, event: AgentEvent) {
    let mut buffers = state.event_buffers.write().await;
    if let Some(buffer) = buffers.get_mut(session_id) {
        buffer.push(event);
    } else {
        // 闸门可能在 run 已结束后发出，此时 buffer 可能已清；建一个临时的
        // 以便 pusher 若仍订阅能收到。多数情况 pusher 已按 interaction 推卡。
        let mut buf = crate::chat::EventBuffer::new();
        buf.push(event);
        buffers.insert(session_id.to_string(), buf);
    }
}

/// 旁路日志：写失败只 warn，不向上传播。
async fn append_event(
    state: &Arc<AppState>,
    task_id: &str,
    kind: &str,
    role: Option<&str>,
    round: Option<i32>,
    operator: Option<&str>,
    detail: Option<&str>,
) -> Result<()> {
    if let Err(e) = state
        .db
        .append_team_event(task_id, kind, role, round, operator, detail)
        .await
    {
        tracing::warn!(task_id, kind, "append_team_event 失败: {e:#}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 单测（纯函数部分）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn dummy_run(role: &str, round: i32, status: &str) -> TeamTaskRun {
        TeamTaskRun {
            id: format!("{role}-{round}"),
            task_id: "t1".into(),
            role: role.into(),
            round,
            thread_id: None,
            status: status.into(),
            verdict: None,
            handoff: None,
            summary: Some(format!("{role} r{round}")),
            dirty_files: Some(1),
            error: None,
            started_at: Utc::now(),
            finished_at: None,
        }
    }

    // ----- normalize_verdict -----

    #[test]
    fn normalize_verdict_none_needs_verdict_is_unknown() {
        assert_eq!(normalize_verdict(None, true), Verdict::Unknown);
    }

    #[test]
    fn normalize_verdict_some_unknown_stays_unknown() {
        assert_eq!(
            normalize_verdict(Some(Verdict::Unknown), true),
            Verdict::Unknown
        );
    }

    #[test]
    fn normalize_verdict_pass_and_reject() {
        assert_eq!(normalize_verdict(Some(Verdict::Pass), true), Verdict::Pass);
        assert_eq!(
            normalize_verdict(Some(Verdict::Reject), true),
            Verdict::Reject
        );
    }

    #[test]
    fn normalize_verdict_developer_forces_pass() {
        // 开发角色 needs_verdict=false：无论 handoff 写什么都 Pass
        assert_eq!(normalize_verdict(None, false), Verdict::Pass);
        assert_eq!(
            normalize_verdict(Some(Verdict::Reject), false),
            Verdict::Pass
        );
        assert_eq!(
            normalize_verdict(Some(Verdict::Unknown), false),
            Verdict::Pass
        );
    }

    // ----- gate_options -----

    #[test]
    fn gate_options_all_boundaries() {
        assert_eq!(
            gate_options(GateBoundary::DevStart),
            vec!["开始修".to_string(), "跳过".to_string()]
        );
        assert_eq!(
            gate_options(GateBoundary::ReviewStart),
            vec!["继续评审".to_string(), "终止".to_string()]
        );
        assert_eq!(
            gate_options(GateBoundary::DevRestart),
            vec!["重新开发".to_string(), "终止".to_string()]
        );
        assert_eq!(
            gate_options(GateBoundary::TestStart),
            vec!["继续测试".to_string(), "终止".to_string()]
        );
    }

    // ----- pick_upstream_run -----

    #[test]
    fn pick_upstream_reviewer_takes_latest_developer() {
        let roles = vec![
            "developer".into(),
            "reviewer".into(),
            "tester".into(),
        ];
        let runs = vec![
            dummy_run("developer", 1, "finished"),
            dummy_run("developer", 2, "finished"),
            dummy_run("reviewer", 1, "running"),
        ];
        let up = pick_upstream_run(&runs, "reviewer", &roles).unwrap();
        assert_eq!(up.role, "developer");
        assert_eq!(up.round, 2);
    }

    #[test]
    fn pick_upstream_tester_takes_latest_reviewer() {
        let roles = vec![
            "developer".into(),
            "reviewer".into(),
            "tester".into(),
        ];
        let runs = vec![
            dummy_run("developer", 1, "finished"),
            dummy_run("reviewer", 1, "finished"),
        ];
        let up = pick_upstream_run(&runs, "tester", &roles).unwrap();
        assert_eq!(up.role, "reviewer");
        assert_eq!(up.round, 1);
    }

    #[test]
    fn pick_upstream_developer_first_round_none() {
        let roles = vec![
            "developer".into(),
            "reviewer".into(),
            "tester".into(),
        ];
        // 首轮开发：还没有评审 finished
        let runs = vec![dummy_run("developer", 1, "running")];
        assert!(pick_upstream_run(&runs, "developer", &roles).is_none());
    }

    #[test]
    fn pick_upstream_developer_round2_takes_reviewer() {
        let roles = vec![
            "developer".into(),
            "reviewer".into(),
            "tester".into(),
        ];
        // 打回场景：开发第 2 轮取评审那轮
        let runs = vec![
            dummy_run("developer", 1, "finished"),
            dummy_run("reviewer", 1, "finished"),
        ];
        let up = pick_upstream_run(&runs, "developer", &roles).unwrap();
        assert_eq!(up.role, "reviewer");
        assert_eq!(up.round, 1);
    }

    #[test]
    fn pick_upstream_empty_runs_none() {
        let roles = vec![
            "developer".into(),
            "reviewer".into(),
            "tester".into(),
        ];
        assert!(pick_upstream_run(&[], "reviewer", &roles).is_none());
        assert!(pick_upstream_run(&[], "developer", &roles).is_none());
    }

    #[test]
    fn pick_upstream_ignores_non_finished() {
        let roles = vec![
            "developer".into(),
            "reviewer".into(),
            "tester".into(),
        ];
        let runs = vec![
            dummy_run("developer", 1, "failed"),
            dummy_run("developer", 2, "running"),
        ];
        assert!(pick_upstream_run(&runs, "reviewer", &roles).is_none());
    }
}

