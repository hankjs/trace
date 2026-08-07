//! 定时任务调度器：cron 触发的系统主动工作入口（agent-os 的"自动化工作流"）。
//!
//! 参考 quant 的作业模型（app/scheduler.py + app/job_log.py）：
//! - 任务定义在代码里（JOB_DEFS 静态注册表），启停状态在 DB（job_states）
//! - 执行记录落 job_runs（旁路日志：写库失败不影响任务本身）
//! - 手动触发与系统调度共用同一执行路径，不绕过任务内部守卫
//! - 每 job 一把并发锁；进程重启遗留的 running 行启动时收尾为 failed
//!
//! 多实例共库时只能一个实例开调度（server.scheduler_enabled，与 monitor 同理）。

pub mod jobs;
pub mod routes;

use crate::AppState;
use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

pub const TRIGGER_SYSTEM: &str = "system";
pub const TRIGGER_MANUAL: &str = "manual";

pub const STATUS_FINISHED: &str = "finished";
pub const STATUS_FAILED: &str = "failed";

/// 调度时区（A 股业务时间均为上海时区）
pub const TZ: Tz = chrono_tz::Asia::Shanghai;
/// 调度循环粒度
const TICK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

type JobHandler = fn(Arc<AppState>) -> Pin<Box<dyn Future<Output = Result<Value>> + Send>>;

/// 任务定义：静态注册，schedule 变更跟随代码发布（与 quant 同一约定）
pub struct JobDef {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// 6 字段 cron 表达式（秒 分 时 日 月 周），上海时区
    pub cron: &'static str,
    /// 调度时间的人类可读描述（admin 页面展示）
    pub schedule_label: &'static str,
    pub handler: JobHandler,
}

pub const JOB_DEFS: &[JobDef] = &[JobDef {
    id: "quant_signal_brief",
    name: "盘后信号简报",
    description: "交易日盘后拉取 quant 今日信号，按绑定用户推飞书单聊简报；无信号保持安静。",
    cron: "0 45 17 * * mon-fri",
    schedule_label: "工作日 17:45（盘后流水线之后）",
    handler: |state| Box::pin(jobs::quant_signal_brief(state)),
}];

pub fn job_def(job_id: &str) -> Option<&'static JobDef> {
    JOB_DEFS.iter().find(|j| j.id == job_id)
}

/// 调度器运行时状态（挂 AppState）
pub struct SchedulerState {
    /// 每 job 一把并发锁（系统调度与手动触发互斥）
    pub locks: HashMap<String, Arc<AtomicBool>>,
    /// 下次执行时间缓存（展示 + 触发判定）
    pub next_runs: RwLock<HashMap<String, DateTime<Tz>>>,
}

impl SchedulerState {
    pub fn new() -> Self {
        Self {
            locks: JOB_DEFS
                .iter()
                .map(|j| (j.id.to_string(), Arc::new(AtomicBool::new(false))))
                .collect(),
            next_runs: RwLock::new(HashMap::new()),
        }
    }
}

/// server 启动时调用：收尾僵尸记录 + 启动调度循环。
pub fn start(state: Arc<AppState>) {
    if !state.config.server.scheduler_enabled {
        tracing::info!("scheduler disabled by config (scheduler_enabled=false), skip");
        return;
    }
    // handy 闸门兜底轮询（30s）：与调度器同开关——多实例共库时只能一个实例
    // 轮询，否则同一应答会被两个实例重复补答（虽有原子应答兜底，但白白双倍请求）。
    jobs::start_handy_gate_poller(state.clone());
    tokio::spawn(async move {
        match state.db.fail_stale_running_job_runs().await {
            Ok(0) => {}
            Ok(n) => tracing::warn!("scheduler: 收尾 {n} 条进程重启遗留的 running 记录"),
            Err(e) => tracing::warn!("scheduler: 收尾僵尸记录失败: {e:#}"),
        }
        // 预填 next_runs
        for job in JOB_DEFS {
            if let Some(next) = compute_next(job) {
                state
                    .scheduler
                    .next_runs
                    .write()
                    .await
                    .insert(job.id.to_string(), next);
            }
        }
        tracing::info!(
            "scheduler started: {}",
            JOB_DEFS.iter().map(|j| j.id).collect::<Vec<_>>().join(", ")
        );

        let mut tick = tokio::time::interval(TICK_INTERVAL);
        loop {
            tick.tick().await;
            for job in JOB_DEFS {
                let enabled = state.db.get_job_enabled(job.id).await.unwrap_or(true);
                if !enabled {
                    continue;
                }
                let next = {
                    let map = state.scheduler.next_runs.read().await;
                    map.get(job.id).copied()
                };
                let Some(next) = next else { continue };
                if TZ.from_utc_datetime(&Utc::now().naive_utc()) >= next {
                    // 到点：先重算下次时间，再异步执行（执行耗时阻塞不了调度）
                    if let Some(following) = compute_next(job) {
                        state
                            .scheduler
                            .next_runs
                            .write()
                            .await
                            .insert(job.id.to_string(), following);
                    }
                    let state = state.clone();
                    tokio::spawn(async move {
                        if let Err(e) = execute_job(state, job, TRIGGER_SYSTEM, None).await {
                            tracing::debug!("scheduler: {} 未执行: {e}", job.id);
                        }
                    });
                }
            }
        }
    });
}

fn compute_next(job: &JobDef) -> Option<DateTime<Tz>> {
    let schedule = cron::Schedule::from_str(job.cron)
        .map_err(|e| tracing::error!("scheduler: {} cron 表达式无效: {e}", job.id))
        .ok()?;
    schedule.upcoming(TZ).next()
}

/// 执行一次任务（系统调度与手动触发共用）。记录 job_runs，旁路容错。
pub async fn execute_job(
    state: Arc<AppState>,
    job: &'static JobDef,
    trigger: &str,
    operator: Option<&str>,
) -> std::result::Result<(), String> {
    let lock = state
        .scheduler
        .locks
        .get(job.id)
        .cloned()
        .ok_or_else(|| format!("任务未注册锁: {}", job.id))?;
    if lock
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("该任务已有执行进行中".to_string());
    }

    // 旁路日志：写库失败不阻塞任务执行
    let run_id = state
        .db
        .create_job_run(job.id, trigger, operator)
        .await
        .map_err(|e| tracing::warn!("scheduler: 写 job_runs 失败: {e:#}"))
        .ok();

    let result = (job.handler)(state.clone()).await;

    if let Some(id) = run_id {
        let (status, result_text, error) = match &result {
            Ok(v) => (STATUS_FINISHED, Some(v.to_string()), None),
            Err(e) => (STATUS_FAILED, None, Some(format!("{e:#}"))),
        };
        if let Err(e) = state
            .db
            .finish_job_run(id, status, result_text.as_deref(), error.as_deref())
            .await
        {
            tracing::warn!("scheduler: 更新 job_runs 失败: {e:#}");
        }
    }

    lock.store(false, Ordering::SeqCst);
    result.map(|_| ()).map_err(|e| format!("{e:#}"))
}
