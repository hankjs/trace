//! 飞书外部 Agent 后端：可在 server bubblewrap 或绑定的 hank-cli 节点运行。

use crate::chat::{ChatTurnHandle, EventBuffer};
use crate::AppState;
use anyhow::{anyhow, bail, Context, Result};
use code_agent::{AgentEvent, FileChange, FileChangeKind, RunStatus};
use hank_db::{NewInteraction, Session};
use hank_provider::ContentBlock;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::ffi::OsStr;
#[cfg(unix)]
use std::fs::File;
use std::io::Read;
#[cfg(unix)]
use std::mem::ManuallyDrop;
#[cfg(unix)]
use std::os::fd::FromRawFd;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const AUTH_ENV_KEYS: &[&str] = &[
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "OPENAI_BASE_URL",
];

pub(crate) const SANDBOX_LAUNCHER_ARG: &str = "--agent-sandbox-launcher";
const SANDBOX_LAUNCHER_BIN: &str = "/opt/hank/current/hank-server";
const SANDBOX_BIN: &str = "/usr/bin/bwrap";
const LAUNCHER_MAGIC: &[u8] = b"HANK_AGENT_ENV_V1\0";
const MAX_LAUNCHER_ENV_COUNT: usize = 32;
const MAX_LAUNCHER_ENV_KEY_BYTES: usize = 128;
const MAX_LAUNCHER_ENV_VALUE_BYTES: usize = 128 * 1024;

const SANDBOX_BASE_ENV_KEYS: &[&str] = &[
    "HOME",
    "CODEX_HOME",
    "CLAUDE_CONFIG_DIR",
    "CARGO_HOME",
    "GIT_OPTIONAL_LOCKS",
    "PATH",
    "RUSTUP_HOME",
    "UV_CACHE_DIR",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "LC_ALL",
];

pub(crate) fn sandbox_launcher_requested() -> bool {
    std::env::args_os().nth(1).as_deref() == Some(OsStr::new(SANDBOX_LAUNCHER_ARG))
}

/// sudo 只看到固定启动命令；本轮凭据从 stdin 前导段读取，剩余 stdin 原样留给 CLI prompt。
#[cfg(unix)]
pub(crate) fn run_sandbox_launcher() -> Result<()> {
    let mut args = std::env::args_os().skip(2);
    let executable = args
        .next()
        .ok_or_else(|| anyhow!("Agent 沙箱启动器缺少可执行文件"))?;
    if executable != OsStr::new(SANDBOX_BIN) {
        bail!("Agent 沙箱启动器只允许执行 {SANDBOX_BIN}");
    }

    // 直接读 fd 0，避免 BufReader 预读并吞掉紧随凭据后的 prompt。
    let mut input = ManuallyDrop::new(unsafe { File::from_raw_fd(0) });
    let environment = read_launcher_environment(&mut *input)?;

    // sudo 继承的环境不进入 Agent；只保留服务端通过白名单协议显式传入的键。
    let inherited: Vec<_> = std::env::vars_os().map(|(key, _)| key).collect();
    for key in inherited {
        std::env::remove_var(key);
    }
    for (key, value) in environment {
        std::env::set_var(key, value);
    }

    let error = std::process::Command::new(&executable).args(args).exec();
    Err(anyhow!("启动 bubblewrap 失败: {error}"))
}

#[cfg(not(unix))]
pub(crate) fn run_sandbox_launcher() -> Result<()> {
    bail!("Agent 沙箱启动器只支持 Unix")
}

fn launcher_env_key_allowed(key: &str) -> bool {
    SANDBOX_BASE_ENV_KEYS.contains(&key)
        || CLAUDE_AUTH_KINDS.contains(&key)
        || CODEX_AUTH_KINDS.contains(&key)
        || CLAUDE_EXTRA_ENV_KEYS.contains(&key)
        || CODEX_EXTRA_ENV_KEYS.contains(&key)
        || matches!(key, "ANTHROPIC_BASE_URL" | "ANTHROPIC_MODEL")
}

fn read_launcher_environment<R: Read>(input: &mut R) -> Result<Vec<(String, String)>> {
    let mut magic = vec![0; LAUNCHER_MAGIC.len()];
    input
        .read_exact(&mut magic)
        .context("读取 Agent 凭据协议头")?;
    if magic != LAUNCHER_MAGIC {
        bail!("Agent 凭据协议头无效");
    }

    let count = read_u16(input)? as usize;
    if count > MAX_LAUNCHER_ENV_COUNT {
        bail!("Agent 凭据变量数量超过上限");
    }
    let mut seen = HashSet::new();
    let mut environment = Vec::with_capacity(count);
    for _ in 0..count {
        let key_len = read_u16(input)? as usize;
        let value_len = read_u32(input)? as usize;
        if key_len == 0 || key_len > MAX_LAUNCHER_ENV_KEY_BYTES {
            bail!("Agent 凭据变量名长度无效");
        }
        if value_len > MAX_LAUNCHER_ENV_VALUE_BYTES {
            bail!("Agent 凭据变量值超过上限");
        }

        let mut key = vec![0; key_len];
        let mut value = vec![0; value_len];
        input
            .read_exact(&mut key)
            .context("读取 Agent 凭据变量名")?;
        input
            .read_exact(&mut value)
            .context("读取 Agent 凭据变量值")?;
        let key = String::from_utf8(key).context("Agent 凭据变量名不是 UTF-8")?;
        let value = String::from_utf8(value).context("Agent 凭据变量值不是 UTF-8")?;
        if !launcher_env_key_allowed(&key) {
            bail!("Agent 启动环境不允许变量 {key}");
        }
        if !seen.insert(key.clone()) {
            bail!("Agent 启动环境包含重复变量 {key}");
        }
        if value.contains('\0') {
            bail!("Agent 启动环境变量 {key} 包含 NUL");
        }
        environment.push((key, value));
    }
    Ok(environment)
}

fn read_u16<R: Read>(input: &mut R) -> Result<u16> {
    let mut bytes = [0; 2];
    input
        .read_exact(&mut bytes)
        .context("读取 Agent 凭据长度")?;
    Ok(u16::from_be_bytes(bytes))
}

fn read_u32<R: Read>(input: &mut R) -> Result<u32> {
    let mut bytes = [0; 4];
    input
        .read_exact(&mut bytes)
        .context("读取 Agent 凭据长度")?;
    Ok(u32::from_be_bytes(bytes))
}

fn encode_launcher_environment(environment: &[(String, String)]) -> Result<Vec<u8>> {
    if environment.len() > MAX_LAUNCHER_ENV_COUNT {
        bail!("Agent 启动环境变量数量超过上限");
    }
    let count = u16::try_from(environment.len()).context("编码 Agent 启动环境数量")?;
    let mut encoded = Vec::new();
    encoded.extend_from_slice(LAUNCHER_MAGIC);
    encoded.extend_from_slice(&count.to_be_bytes());
    let mut seen = HashSet::new();
    for (key, value) in environment {
        if !launcher_env_key_allowed(key) {
            bail!("Agent 启动环境不允许变量 {key}");
        }
        if !seen.insert(key.as_str()) {
            bail!("Agent 启动环境包含重复变量 {key}");
        }
        if key.is_empty() || key.len() > MAX_LAUNCHER_ENV_KEY_BYTES {
            bail!("Agent 启动环境变量名长度无效");
        }
        if value.len() > MAX_LAUNCHER_ENV_VALUE_BYTES || value.contains('\0') {
            bail!("Agent 启动环境变量 {key} 的值无效");
        }
        let key_len = u16::try_from(key.len()).context("编码 Agent 启动环境变量名")?;
        let value_len = u32::try_from(value.len()).context("编码 Agent 启动环境变量值")?;
        encoded.extend_from_slice(&key_len.to_be_bytes());
        encoded.extend_from_slice(&value_len.to_be_bytes());
        encoded.extend_from_slice(key.as_bytes());
        encoded.extend_from_slice(value.as_bytes());
    }
    Ok(encoded)
}

/// 各后端允许的凭据环境变量名。第三方 Anthropic 中转通常只认 ANTHROPIC_AUTH_TOKEN，
/// 官方 key 用 ANTHROPIC_API_KEY，订阅登录用 CLAUDE_CODE_OAUTH_TOKEN，三者不可混用。
pub(crate) const CLAUDE_AUTH_KINDS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
];
pub(crate) const CODEX_AUTH_KINDS: &[&str] = &["OPENAI_API_KEY"];

/// admin 可配置的附加环境变量白名单。只允许模型与输出上限这类无副作用的字段，
/// 防止把任意环境变量注入到沙箱里的 CLI 进程。
pub(crate) const CLAUDE_EXTRA_ENV_KEYS: &[&str] = &[
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
];
pub(crate) const CODEX_EXTRA_ENV_KEYS: &[&str] = &[];

/// 返回后端允许的 auth_kind 与附加环境变量白名单，未知后端返回 None。
pub(crate) fn backend_env_whitelist(
    backend: &str,
) -> Option<(&'static [&'static str], &'static [&'static str])> {
    match backend {
        "claude" => Some((CLAUDE_AUTH_KINDS, CLAUDE_EXTRA_ENV_KEYS)),
        "codex" => Some((CODEX_AUTH_KINDS, CODEX_EXTRA_ENV_KEYS)),
        _ => None,
    }
}

/// 凭据的实际来源，admin 用它显示「当前生效的是库里的配置还是服务器上的环境文件」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AuthSource {
    /// admin 在 agent_cli_configs 里配置的行
    Db,
    /// 服务器 /opt/hank/agent-cli.env
    Env,
    /// 复用 server 里已启用的 provider 记录
    Provider,
}

#[derive(Default)]
struct CliRunState {
    final_text: String,
    input_tokens: u32,
    output_tokens: u32,
    thread_id: Option<String>,
    changed_files: HashSet<String>,
    failed_message: Option<String>,
    /// CLI 自报的实际模型名。第三方端点下由 CLI 决定默认模型，环境变量里不一定有，
    /// 只能从流式事件里拿；用于回写 sessions.model 让 admin 能看出跑的是哪个模型。
    model: Option<String>,
}

#[derive(Default)]
struct CliAuth {
    env: Vec<(&'static str, String)>,
    model: Option<String>,
    base_url: Option<String>,
}

/// 把 admin 存在库里的配置行翻译成 CliAuth。纯函数，不碰环境变量和数据库，
/// 便于单测优先级与白名单过滤。返回 None 表示这行不可用（没有凭据），
/// 调用方据此继续往下走 env / provider 兜底。
fn auth_from_db_config(backend: &str, config: &hank_db::AgentCliProfileRecord) -> Option<CliAuth> {
    let api_key = config.api_key.trim();
    if api_key.is_empty() {
        return None;
    }
    let (auth_kinds, extra_keys) = backend_env_whitelist(backend)?;
    // auth_kind 必须命中白名单，才能拿到 auth.env 需要的 'static key。
    let auth_key = auth_kinds
        .iter()
        .find(|candidate| **candidate == config.auth_kind.trim())
        .copied()
        // 历史行或 codex 可能没填 auth_kind，退回该后端的默认凭据变量名。
        .or_else(|| auth_kinds.first().copied())?;

    let mut auth = CliAuth::default();
    auth.env.push((auth_key, api_key.to_string()));

    let base_url = config.base_url.trim();
    if !base_url.is_empty() {
        match backend {
            // Codex 第三方端点通过命令行组装完整 custom Responses provider。
            "codex" => auth.base_url = Some(base_url.to_string()),
            _ => auth.env.push(("ANTHROPIC_BASE_URL", base_url.to_string())),
        }
    }

    let model = config.model.trim();
    if !model.is_empty() {
        auth.model = Some(model.to_string());
        if backend == "claude" {
            // Claude 除了 --model 参数也读这个环境变量，保持与 env 文件路径一致。
            auth.env.push(("ANTHROPIC_MODEL", model.to_string()));
        }
    }

    // extra_env 是 admin 填的 JSON 对象，只放行白名单内的键。
    if let Ok(serde_json::Value::Object(map)) =
        serde_json::from_str::<serde_json::Value>(&config.extra_env)
    {
        for key in extra_keys {
            if let Some(value) = map.get(*key).and_then(serde_json::Value::as_str) {
                if !value.trim().is_empty() {
                    auth.env.push((key, value.trim().to_string()));
                }
            }
        }
    }
    Some(auth)
}

pub async fn run_cli_turn(
    state: &Arc<AppState>,
    session_id: &str,
    session: Option<Session>,
    content: Vec<ContentBlock>,
    backend: &str,
) -> Result<ChatTurnHandle> {
    Uuid::parse_str(session_id).context("外部 Agent session_id 不是 UUID")?;
    let session = session.ok_or_else(|| anyhow!("外部 Agent 会话不存在"))?;
    let metadata = parse_metadata(session.metadata.as_deref());
    let agent_kind = metadata["agent_kind"].as_str().unwrap_or("general_task");
    if agent_kind == "conversation" {
        bail!("纯对话必须由无工具的 native 后端执行");
    }
    let agent_location = metadata["agent_location"].as_str();
    let is_client_only = agent_location == Some("client");
    // client-only 或已绑定 exec_client 的会话必须走远程 agent_run；不得回退 server bubblewrap。
    if is_client_only || session.exec_client_id.is_some() {
        let client_id = session
            .exec_client_id
            .clone()
            .or_else(|| {
                metadata["exec_client_id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .map(str::to_string)
            })
            .ok_or_else(|| {
                anyhow!("client-only 会话缺少 exec_client_id，无法在本机 hank-cli 执行")
            })?;
        return run_remote_cli_turn(
            state, session_id, session, content, backend, &client_id, agent_kind,
        )
        .await;
    }
    // server-only bubblewrap 路径保留编译与非飞书调用兼容；飞书不再创建此类会话。
    let work_dir = session
        .work_dir
        .as_deref()
        .ok_or_else(|| anyhow!("外部代码 Agent 缺少工作目录"))?;
    let work_dir = tokio::fs::canonicalize(work_dir)
        .await
        .context("工作目录不存在")?;
    validate_workspace(state, &work_dir)?;
    validate_runtime(state, backend).await?;
    let auth = resolve_cli_auth(state, backend).await?;

    let user_text = content_text(&content);
    if user_text.trim().is_empty() {
        bail!("外部 Agent 暂不支持没有文本说明的消息");
    }
    let prompt = agent_prompt(&user_text, agent_kind);
    let state_dir = prepare_state_dir(state, session_id).await?;
    let git_link = prepare_git_link(state, &work_dir, &state_dir).await?;

    {
        let mut buffers = state.event_buffers.write().await;
        buffers.insert(session_id.to_string(), EventBuffer::new());
    }
    let event_rx = {
        let buffers = state.event_buffers.read().await;
        buffers
            .get(session_id)
            .expect("event buffer just inserted")
            .tx
            .subscribe()
    };
    let cancel_token = CancellationToken::new();
    state
        .active_tasks
        .write()
        .await
        .insert(session_id.to_string(), cancel_token.clone());

    let state_task = state.clone();
    let session_id_task = session_id.to_string();
    let backend = backend.to_string();
    let work_dir_task = work_dir.clone();
    tokio::spawn(async move {
        let result = execute_turn(
            &state_task,
            &session_id_task,
            &session,
            &backend,
            &work_dir_task,
            &state_dir,
            git_link.as_ref(),
            &auth,
            &user_text,
            &prompt,
            cancel_token.clone(),
        )
        .await;
        if let Err(error) = result {
            tracing::error!(session_id = %session_id_task, backend, "external agent failed: {error:#}");
            emit(
                &state_task,
                &session_id_task,
                AgentEvent::RunFailed {
                    run_id: session_id_task.clone(),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    message: format!("{error:#}"),
                },
            )
            .await;
            emit(&state_task, &session_id_task, AgentEvent::TurnComplete).await;
        }
        state_task
            .active_tasks
            .write()
            .await
            .remove(&session_id_task);
        if let Some(buffer) = state_task
            .event_buffers
            .write()
            .await
            .get_mut(&session_id_task)
        {
            buffer.completed = true;
        }
    });

    Ok(ChatTurnHandle { event_rx })
}

async fn run_remote_cli_turn(
    state: &Arc<AppState>,
    session_id: &str,
    session: Session,
    content: Vec<ContentBlock>,
    backend: &str,
    client_id: &str,
    agent_kind: &str,
) -> Result<ChatTurnHandle> {
    let user_id = session
        .user_id
        .clone()
        .ok_or_else(|| anyhow!("本机 Agent 会话缺少用户"))?;
    if !crate::remote_exec::is_client_online(state, &user_id, client_id).await {
        bail!("绑定的 hank-cli 节点不在线；请在对应电脑启动 hank-cli 后重试");
    }
    if !crate::remote_exec::client_reports_backend(state, &user_id, client_id, backend).await {
        bail!(
            "绑定的 hank-cli 节点未上报 {backend} 能力；请检查本机 agent_backends 后重试，不会切换节点或回退 server"
        );
    }
    // 展示用节点注册 work_dir；绝不回退 session.work_dir（可能是 wananyun/server worktree）。
    // 实际 agent_run 下发的 cwd 仍为 JSON null，由 hank-cli 使用本机注册目录。
    let client_work_dir = state
        .db
        .get_client_agent(&user_id, client_id)
        .await
        .ok()
        .flatten()
        .and_then(|client| client.work_dir);
    let display_cwd = client_only_display_cwd(client_work_dir.as_deref());
    let user_text = content_text(&content);
    if user_text.trim().is_empty() {
        bail!("本机 Agent 暂不支持没有文本说明的消息");
    }
    // 闸门判定：与 server_agent.enabled 无关，只看 task_gate_enabled + 会话条件。
    let metadata = parse_metadata(session.metadata.as_deref());
    let source = metadata["source"].as_str();
    let existing_thread = metadata["agent_thread_id"].as_str();
    let gate_mode = should_gate_turn(
        state.config.server_agent.task_gate_enabled,
        agent_kind,
        source,
        existing_thread,
    );
    let prompt = if gate_mode {
        local_agent_analysis_prompt(&user_text, agent_kind)
    } else {
        local_agent_prompt(&user_text, agent_kind)
    };

    {
        let mut buffers = state.event_buffers.write().await;
        buffers.insert(session_id.to_string(), EventBuffer::new());
    }
    let event_rx = {
        let buffers = state.event_buffers.read().await;
        buffers
            .get(session_id)
            .expect("event buffer just inserted")
            .tx
            .subscribe()
    };
    let cancel_token = CancellationToken::new();
    state
        .active_tasks
        .write()
        .await
        .insert(session_id.to_string(), cancel_token.clone());

    let state_task = state.clone();
    let session_id_task = session_id.to_string();
    let backend = backend.to_string();
    let client_id = client_id.to_string();
    let agent_kind = agent_kind.to_string();
    tokio::spawn(async move {
        let result = execute_remote_turn(
            &state_task,
            &session_id_task,
            &session,
            &backend,
            &client_id,
            &user_id,
            &display_cwd,
            &user_text,
            &prompt,
            gate_mode,
            &agent_kind,
            cancel_token.clone(),
        )
        .await;
        if let Err(error) = result {
            tracing::error!(session_id = %session_id_task, backend, "remote cli agent failed: {error:#}");
            emit(
                &state_task,
                &session_id_task,
                AgentEvent::RunFailed {
                    run_id: session_id_task.clone(),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    message: format!("{error:#}"),
                },
            )
            .await;
            emit(&state_task, &session_id_task, AgentEvent::TurnComplete).await;
        }
        state_task
            .active_tasks
            .write()
            .await
            .remove(&session_id_task);
        if let Some(buffer) = state_task
            .event_buffers
            .write()
            .await
            .get_mut(&session_id_task)
        {
            buffer.completed = true;
        }
    });

    Ok(ChatTurnHandle { event_rx })
}

#[derive(Debug, Deserialize)]
struct RemoteAgentResult {
    #[serde(default)]
    exit_code: Option<i32>,
    #[serde(default)]
    cancelled: bool,
    #[serde(default)]
    output_limited: bool,
    #[serde(default)]
    stdout: String,
    #[serde(default)]
    stderr: String,
}

#[allow(clippy::too_many_arguments)]
async fn execute_remote_turn(
    state: &Arc<AppState>,
    session_id: &str,
    session: &Session,
    backend: &str,
    client_id: &str,
    user_id: &str,
    work_dir: &str,
    user_text: &str,
    prompt: &str,
    // 第一轮闸门模式：成功结束时落 task_gate 交互单，不 emit RunCompleted。
    gate_mode: bool,
    agent_kind: &str,
    cancel_token: CancellationToken,
) -> Result<()> {
    let run_id = Uuid::new_v4().to_string();
    emit(
        state,
        session_id,
        AgentEvent::RunStarted {
            run_id: run_id.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            cwd: Some(work_dir.to_string()),
            model: format!("{backend}@hank-cli"),
            permission_mode: "client-workspace".to_string(),
            tools: vec![backend.to_string()],
        },
    )
    .await;

    let parent_id = session.active_leaf_id.as_deref();
    let user_content = serde_json::json!([{"type":"text","text":user_text}]);
    let user_message_id = state
        .db
        .save_message(
            session_id,
            "user",
            &user_content,
            chrono::Utc::now(),
            parent_id,
        )
        .await
        .context("保存本机 Agent 用户消息")?;
    state
        .db
        .update_active_leaf(session_id, &user_message_id)
        .await
        .context("更新本机 Agent 用户消息游标")?;
    if session.title.trim().is_empty() {
        let title: String = user_text.chars().take(50).collect();
        let _ = state.db.update_session_title(session_id, &title).await;
    }

    let metadata = parse_metadata(session.metadata.as_deref());
    let previous_thread = metadata["agent_thread_id"].as_str();
    // cwd 置 null：由 hank-cli 使用注册 work_dir，避免把 server 绝对路径传到本机。
    let input = serde_json::json!({
        "backend": backend,
        "prompt": prompt,
        "cwd": serde_json::Value::Null,
        "thread_id": previous_thread,
        "model": serde_json::Value::Null,
    });
    let mut remote = crate::remote_exec::start_agent_run(state, user_id, client_id, input).await?;
    let remote_request_id = remote.request_id.clone();
    let deadline = tokio::time::sleep(Duration::from_secs(
        state.config.server_agent.agent_timeout_secs.max(1),
    ));
    tokio::pin!(deadline);
    let mut run_state = CliRunState::default();
    let auth = CliAuth::default();
    let mut terminal = Terminal::Completed;
    let mut tool_result: Option<crate::remote_exec::ToolCallResult> = None;
    let mut events_closed = false;
    let mut offline_error: Option<String> = None;

    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                terminal = Terminal::Cancelled;
                if let Err(error) = crate::remote_exec::cancel_agent_run(
                    state,
                    user_id,
                    client_id,
                    &remote_request_id,
                ).await {
                    tracing::warn!(session_id, client_id, "取消本机 Agent 失败: {error:#}");
                    // 节点已离线时仍标记取消，终态卡片会说明取消请求可能未送达。
                    if offline_error.is_none() {
                        offline_error = Some(format!("取消请求未确认：{error:#}"));
                    }
                }
                break;
            }
            _ = &mut deadline => {
                terminal = Terminal::TimedOut;
                if let Err(error) = crate::remote_exec::cancel_agent_run(
                    state,
                    user_id,
                    client_id,
                    &remote_request_id,
                ).await {
                    tracing::warn!(session_id, client_id, "超时后取消本机 Agent 失败: {error:#}");
                }
                break;
            }
            event = remote.event_rx.recv(), if !events_closed => {
                match event {
                    Some(event) => {
                        handle_remote_event(state, session_id, backend, &auth, event, &mut run_state).await;
                    }
                    None => {
                        // 事件通道关闭后不再轮询，避免 busy-loop；仍等待 result 或超时。
                        events_closed = true;
                    }
                }
            }
            result = &mut remote.result_rx => {
                match result {
                    Ok(value) => tool_result = Some(value),
                    Err(_) => {
                        offline_error = Some(
                            "hank-cli 结果通道中断（节点可能已离线或进程异常退出）".to_string(),
                        );
                    }
                }
                break;
            }
            _ = tokio::time::sleep(Duration::from_secs(5)) => {
                if !crate::remote_exec::is_client_online(state, user_id, client_id).await {
                    offline_error = Some(
                        "绑定的 hank-cli 节点在任务执行期间离线".to_string(),
                    );
                    // 短窗口内若结果已在路上则仍接收；否则明确失败，不静默当成功。
                    match tokio::time::timeout(Duration::from_secs(8), &mut remote.result_rx)
                        .await
                    {
                        Ok(Ok(value)) => tool_result = Some(value),
                        Ok(Err(_)) => {
                            offline_error = Some(
                                "hank-cli 结果通道中断（节点可能已离线）".to_string(),
                            );
                        }
                        Err(_) => {}
                    }
                    break;
                }
            }
        }
    }
    // hank-cli 会在所有逐行事件上报完成后才回传 tool-result；当两类通道同时
    // ready 时，先收到终态也必须消费已入队事件，避免漏掉 thread_id/model。
    while let Ok(event) = remote.event_rx.try_recv() {
        handle_remote_event(state, session_id, backend, &auth, event, &mut run_state).await;
    }
    crate::remote_exec::cleanup_agent_run(state, user_id, &remote_request_id).await;

    if terminal == Terminal::Completed && tool_result.is_none() {
        let message = offline_error.unwrap_or_else(|| {
            "hank-cli 未返回 Agent 结果（节点可能已离线或结果超时）".to_string()
        });
        emit_failed(state, session_id, &run_id, &message).await;
        if let Err(error) = state.db.touch_session(session_id).await {
            tracing::warn!(session_id, "touch remote cli session failed: {error:#}");
        }
        emit(state, session_id, AgentEvent::TurnComplete).await;
        return Ok(());
    }

    let parsed = tool_result
        .as_ref()
        .and_then(|result| serde_json::from_str::<RemoteAgentResult>(&result.content).ok());
    if terminal == Terminal::Completed
        && parsed.as_ref().is_some_and(|result| result.output_limited)
    {
        terminal = Terminal::OutputLimit;
    }
    if run_state.final_text.trim().is_empty() {
        if let Some(parsed) = parsed.as_ref() {
            run_state.final_text = extract_final_text(&parsed.stdout);
        }
    }
    if let Some(thread_id) = run_state.thread_id.as_deref() {
        persist_thread_id(state, session_id, thread_id).await?;
    }
    let resolved_model = run_state.model.clone().unwrap_or_default();
    if session.provider != backend || session.model != resolved_model {
        if let Err(error) = state
            .db
            .update_session_provider_model(session_id, backend, &resolved_model)
            .await
        {
            tracing::warn!(
                session_id,
                backend,
                "回写本机 Agent provider/model 失败: {error:#}"
            );
        }
    }

    let remote_failed = tool_result.as_ref().is_some_and(|result| result.is_error)
        || parsed.as_ref().is_some_and(|result| {
            result.cancelled || result.exit_code.is_some_and(|code| code != 0)
        })
        || offline_error.is_some();
    match terminal {
        Terminal::Cancelled => {
            if let Some(extra) = offline_error.as_deref() {
                tracing::warn!(session_id, client_id, "本机 Agent 取消时附加信息: {extra}");
            }
            if !gate_mode {
                finalize_open_task_gate(state, session_id, "failed", None, Some("任务已取消"))
                    .await;
            }
            emit(
                state,
                session_id,
                AgentEvent::RunCancelled {
                    run_id,
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    file_changes: file_changes(&run_state),
                    permission_denials: vec![],
                },
            )
            .await;
        }
        Terminal::TimedOut => {
            let message = offline_error
                .clone()
                .unwrap_or_else(|| "本机 Agent 执行超时".to_string());
            if !gate_mode {
                finalize_open_task_gate(state, session_id, "failed", None, Some(message.as_str()))
                    .await;
            }
            emit_failed(state, session_id, &run_id, &message).await;
        }
        Terminal::OutputLimit => {
            persist_partial_before_failure(
                state,
                session_id,
                &user_message_id,
                &run_state.final_text,
            )
            .await;
            if !gate_mode {
                finalize_open_task_gate(
                    state,
                    session_id,
                    "failed",
                    None,
                    Some("本机 Agent 输出超过整流安全上限"),
                )
                .await;
            }
            emit_failed(
                state,
                session_id,
                &run_id,
                "本机 Agent 输出超过整流安全上限（已保留截断前内容）",
            )
            .await;
        }
        Terminal::Completed if !remote_failed && run_state.failed_message.is_none() => {
            let final_text = if run_state.final_text.trim().is_empty() {
                "任务已完成，本机 Agent 未返回文本摘要。".to_string()
            } else {
                run_state.final_text.trim().to_string()
            };
            let assistant_content = serde_json::json!([{"type":"text","text":final_text}]);
            let assistant_id = state
                .db
                .save_message(
                    session_id,
                    "assistant",
                    &assistant_content,
                    chrono::Utc::now(),
                    Some(&user_message_id),
                )
                .await?;
            state
                .db
                .update_active_leaf(session_id, &assistant_id)
                .await?;
            emit(
                state,
                session_id,
                AgentEvent::Metrics {
                    input_tokens: run_state.input_tokens,
                    output_tokens: run_state.output_tokens,
                    latency_ms: 0,
                    model: if resolved_model.is_empty() {
                        backend.to_string()
                    } else {
                        resolved_model
                    },
                    provider: backend.to_string(),
                    phase: Some("remote_cli".to_string()),
                },
            )
            .await;

            // 闸门第一轮：有 thread_id 才落 task_gate；没有则无法 resume，退回直接完成。
            // 绝不 emit RunCompleted——pusher 会把进度卡刷成绿色「已完成」，用户误以为事情做完了。
            let gated = if gate_mode {
                match run_state.thread_id.as_deref() {
                    Some(thread_id) => {
                        match finish_as_task_gate(
                            state,
                            session_id,
                            user_id,
                            client_id,
                            backend,
                            agent_kind,
                            user_text,
                            &final_text,
                            thread_id,
                        )
                        .await
                        {
                            Ok(()) => true,
                            Err(error) => {
                                tracing::warn!(
                                    session_id,
                                    "task_gate 落单失败，退回直接完成: {error:#}"
                                );
                                false
                            }
                        }
                    }
                    None => {
                        tracing::warn!(
                            session_id,
                            "闸门第一轮未拿到 thread_id，退回直接完成（无法 resume）"
                        );
                        false
                    }
                }
            } else {
                false
            };

            if !gated {
                // 第二轮（或非闸门轮）正常完成：若有 executing 的 task_gate 交互单，标 done。
                finalize_open_task_gate(state, session_id, "done", Some(final_text.as_str()), None)
                    .await;
                emit(
                    state,
                    session_id,
                    AgentEvent::RunCompleted {
                        run_id,
                        timestamp: chrono::Utc::now().to_rfc3339(),
                        status: RunStatus::Success,
                        input_tokens: run_state.input_tokens,
                        output_tokens: run_state.output_tokens,
                        summary: final_text,
                        permission_denials: vec![],
                        file_changes: file_changes(&run_state),
                    },
                )
                .await;
            }
        }
        Terminal::Completed => {
            let message = run_state
                .failed_message
                .filter(|message| !message.trim().is_empty())
                .or_else(|| {
                    parsed.as_ref().and_then(|result| {
                        let stderr = result.stderr.trim();
                        (!stderr.is_empty()).then(|| stderr.to_string())
                    })
                })
                .or_else(|| {
                    parsed.is_none().then(|| {
                        tool_result
                            .as_ref()
                            .map(|result| result.content.trim().to_string())
                            .unwrap_or_default()
                    })
                })
                .filter(|message| !message.is_empty())
                .unwrap_or_else(|| {
                    parsed
                        .as_ref()
                        .and_then(|result| result.exit_code)
                        .map(|code| format!("本机 {backend} 异常退出，状态码 {code}"))
                        .unwrap_or_else(|| format!("本机 {backend} 执行失败"))
                });
            let message = truncate(&message, 4000);
            if !gate_mode {
                finalize_open_task_gate(state, session_id, "failed", None, Some(message.as_str()))
                    .await;
            }
            emit_failed(state, session_id, &run_id, &message).await;
        }
    }
    if let Err(error) = state.db.touch_session(session_id).await {
        tracing::warn!(session_id, "touch remote cli session failed: {error:#}");
    }
    emit(state, session_id, AgentEvent::TurnComplete).await;
    Ok(())
}

async fn handle_remote_event(
    state: &Arc<AppState>,
    session_id: &str,
    backend: &str,
    auth: &CliAuth,
    event: serde_json::Value,
    run_state: &mut CliRunState,
) {
    let Some(line) = event["line"].as_str() else {
        return;
    };
    let stream = event["stream"].as_str().unwrap_or("stdout");
    if stream != "stderr" {
        handle_json_line(state, session_id, backend, auth, line, run_state).await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_turn(
    state: &Arc<AppState>,
    session_id: &str,
    session: &Session,
    backend: &str,
    work_dir: &Path,
    state_dir: &Path,
    git_link: Option<&GitLink>,
    auth: &CliAuth,
    user_text: &str,
    prompt: &str,
    cancel_token: CancellationToken,
) -> Result<()> {
    let run_id = Uuid::new_v4().to_string();
    // 已知模型名时优先展示 "claude/claude-opus-4-6" 这种形式，让 trace 里能看出后端和模型；
    // 未显式配置模型时只能先报后端名，等 CLI 首个事件自报模型后再回写 sessions.model。
    let display_model = match auth.model.as_deref() {
        Some(model) => format!("{backend}/{model}"),
        None => backend.to_string(),
    };
    emit(
        state,
        session_id,
        AgentEvent::RunStarted {
            run_id: run_id.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            cwd: Some(work_dir.display().to_string()),
            model: display_model,
            permission_mode: "external-bwrap".to_string(),
            tools: vec![backend.to_string()],
        },
    )
    .await;

    let parent_id = session.active_leaf_id.as_deref();
    let user_content = serde_json::json!([{"type":"text","text":user_text}]);
    let user_message_id = state
        .db
        .save_message(
            session_id,
            "user",
            &user_content,
            chrono::Utc::now(),
            parent_id,
        )
        .await
        .context("保存外部 Agent 用户消息")?;
    state
        .db
        .update_active_leaf(session_id, &user_message_id)
        .await
        .context("更新外部 Agent 用户消息游标")?;
    if session.title.trim().is_empty() {
        let title: String = user_text.chars().take(50).collect();
        let _ = state.db.update_session_title(session_id, &title).await;
    }

    let metadata = parse_metadata(session.metadata.as_deref());
    let previous_thread = metadata["agent_thread_id"].as_str();
    let CliCommand {
        mut command,
        launcher_environment,
    } = build_command(
        state,
        backend,
        work_dir,
        state_dir,
        git_link,
        auth,
        previous_thread,
    )?;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().context("启动外部 Agent 进程")?;
    let mut stdin = child.stdin.take().context("外部 Agent stdin 不可用")?;
    let launcher_header = encode_launcher_environment(&launcher_environment)?;
    stdin
        .write_all(&launcher_header)
        .await
        .context("发送外部 Agent 凭据")?;
    stdin
        .write_all(prompt.as_bytes())
        .await
        .context("发送外部 Agent prompt")?;
    drop(stdin);
    let stdout = child.stdout.take().context("外部 Agent stdout 不可用")?;
    let stderr = child.stderr.take().context("外部 Agent stderr 不可用")?;
    let stderr_limit = state
        .config
        .server_agent
        .agent_output_limit_bytes
        .min(256 * 1024);
    let stderr_task = tokio::spawn(read_limited(stderr, stderr_limit));
    let stdout_limit = state.config.server_agent.agent_output_limit_bytes;
    let mut lines = BufReader::new(stdout)
        .take(stdout_limit.saturating_add(1) as u64)
        .lines();
    let deadline = tokio::time::sleep(Duration::from_secs(
        state.config.server_agent.agent_timeout_secs,
    ));
    tokio::pin!(deadline);
    let mut run_state = CliRunState::default();
    let mut output_bytes = 0usize;
    let mut terminal = Terminal::Completed;

    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                terminal = Terminal::Cancelled;
                terminate_process_group(&mut child).await;
                break;
            }
            _ = &mut deadline => {
                terminal = Terminal::TimedOut;
                terminate_process_group(&mut child).await;
                break;
            }
            line = lines.next_line() => {
                match line.context("读取外部 Agent JSONL")? {
                    Some(line) => {
                        output_bytes = output_bytes.saturating_add(line.len() + 1);
                        if output_bytes > stdout_limit {
                            terminal = Terminal::OutputLimit;
                            terminate_process_group(&mut child).await;
                            break;
                        }
                        handle_json_line(
                            state,
                            session_id,
                            backend,
                            auth,
                            &line,
                            &mut run_state,
                        )
                        .await;
                    }
                    None => break,
                }
            }
        }
    }

    let status = wait_after_output(&mut child).await;
    let stderr = tokio::time::timeout(Duration::from_secs(2), stderr_task)
        .await
        .ok()
        .and_then(|joined| joined.ok())
        .and_then(|read| read.ok())
        .unwrap_or_default();

    // 回写实际执行的后端与模型：provider 固定为后端名（codex / claude），model 取 CLI
    // 自报值，拿不到时退回环境变量里配置的模型。终止方式不影响这次记录。
    let resolved_model = run_state
        .model
        .clone()
        .or_else(|| auth.model.clone())
        .unwrap_or_default();
    if session.provider != backend || session.model != resolved_model {
        if let Err(error) = state
            .db
            .update_session_provider_model(session_id, backend, &resolved_model)
            .await
        {
            tracing::warn!(
                session_id,
                backend,
                "回写外部 Agent provider/model 失败: {error:#}"
            );
        }
    }

    match terminal {
        Terminal::Cancelled => {
            emit(
                state,
                session_id,
                AgentEvent::RunCancelled {
                    run_id,
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    file_changes: file_changes(&run_state),
                    permission_denials: vec![],
                },
            )
            .await;
        }
        Terminal::TimedOut => {
            emit_failed(state, session_id, &run_id, "外部 Agent 执行超时").await;
        }
        Terminal::OutputLimit => {
            persist_partial_before_failure(
                state,
                session_id,
                &user_message_id,
                &redact_secrets(&run_state.final_text, auth),
            )
            .await;
            emit_failed(
                state,
                session_id,
                &run_id,
                "外部 Agent 输出超过整流安全上限（已保留截断前内容）",
            )
            .await;
        }
        Terminal::Completed
            if status.as_ref().is_some_and(|status| status.success())
                && run_state.failed_message.is_none() =>
        {
            if let Some(thread_id) = run_state.thread_id.as_deref() {
                persist_thread_id(state, session_id, thread_id).await?;
            }
            let final_text = if run_state.final_text.trim().is_empty() {
                "任务已完成，外部 Agent 未返回文本摘要。".to_string()
            } else {
                redact_secrets(run_state.final_text.trim(), auth)
            };
            let assistant_content = serde_json::json!([{"type":"text","text":final_text}]);
            let assistant_id = state
                .db
                .save_message(
                    session_id,
                    "assistant",
                    &assistant_content,
                    chrono::Utc::now(),
                    Some(&user_message_id),
                )
                .await?;
            state
                .db
                .update_active_leaf(session_id, &assistant_id)
                .await?;
            emit(
                state,
                session_id,
                AgentEvent::Metrics {
                    input_tokens: run_state.input_tokens,
                    output_tokens: run_state.output_tokens,
                    latency_ms: 0,
                    model: if resolved_model.is_empty() {
                        backend.to_string()
                    } else {
                        resolved_model.clone()
                    },
                    provider: backend.to_string(),
                    phase: Some("external_cli".to_string()),
                },
            )
            .await;
            emit(
                state,
                session_id,
                AgentEvent::RunCompleted {
                    run_id,
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    status: RunStatus::Success,
                    input_tokens: run_state.input_tokens,
                    output_tokens: run_state.output_tokens,
                    summary: final_text,
                    permission_denials: vec![],
                    file_changes: file_changes(&run_state),
                },
            )
            .await;
        }
        Terminal::Completed => {
            let raw_message = run_state
                .failed_message
                .unwrap_or_else(|| sanitize_error(&stderr, status.as_ref()));
            let message = redact_secrets(&raw_message, auth);
            emit_failed(state, session_id, &run_id, &message).await;
        }
    }
    if let Err(error) = state.db.touch_session(session_id).await {
        tracing::warn!(session_id, "touch external agent session failed: {error:#}");
    }
    emit(state, session_id, AgentEvent::TurnComplete).await;
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Terminal {
    Completed,
    Cancelled,
    TimedOut,
    OutputLimit,
}

#[derive(Debug)]
struct GitLink {
    file: PathBuf,
    common_dir: PathBuf,
}

struct CliCommand {
    command: Command,
    launcher_environment: Vec<(String, String)>,
}

fn build_command(
    state: &Arc<AppState>,
    backend: &str,
    work_dir: &Path,
    state_dir: &Path,
    git_link: Option<&GitLink>,
    auth: &CliAuth,
    previous_thread: Option<&str>,
) -> Result<CliCommand> {
    let cfg = &state.config.server_agent;
    let executable = match backend {
        "codex" => Path::new(&cfg.agent_cli_root).join("codex/current/bin/codex"),
        "claude" => Path::new(&cfg.agent_cli_root).join("claude/current/bin/claude"),
        _ => bail!("不支持的外部 Agent 后端: {backend}"),
    };
    let mut launcher_environment = vec![
        ("HOME".to_string(), "/agent-home".to_string()),
        ("CODEX_HOME".to_string(), "/agent-home/.codex".to_string()),
        (
            "CLAUDE_CONFIG_DIR".to_string(),
            "/agent-home/.claude".to_string(),
        ),
        (
            "CARGO_HOME".to_string(),
            "/agent-home/.cargo-cache".to_string(),
        ),
        ("GIT_OPTIONAL_LOCKS".to_string(), "0".to_string()),
        (
            "RUSTUP_HOME".to_string(),
            format!("/home/{}/.rustup", cfg.execution_user),
        ),
        (
            "UV_CACHE_DIR".to_string(),
            "/agent-home/.uv-cache".to_string(),
        ),
        (
            "PATH".to_string(),
            format!(
                "{}/codex/current/bin:{}/claude/current/bin:/home/{}/.cargo/bin:/home/{}/.local/bin:/usr/local/bin:/usr/bin:/bin",
                cfg.agent_cli_root, cfg.agent_cli_root, cfg.execution_user, cfg.execution_user
            ),
        ),
        ("USER".to_string(), cfg.execution_user.clone()),
        ("LOGNAME".to_string(), cfg.execution_user.clone()),
        ("SHELL".to_string(), "/bin/bash".to_string()),
        ("LANG".to_string(), "C.UTF-8".to_string()),
        ("LC_ALL".to_string(), "C.UTF-8".to_string()),
    ];
    launcher_environment.extend(
        auth.env
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone())),
    );

    let mut command = sandbox_launcher_command(&cfg.execution_user, &cfg.agent_sandbox_bin);
    command.args([
        "--die-with-parent",
        "--new-session",
        "--unshare-pid",
        "--ro-bind",
        "/",
        "/",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--tmpfs",
        "/tmp",
        "--bind",
    ]);
    command.arg(work_dir).arg("/workspace");
    command.arg("--bind");
    command.arg(state_dir).arg("/agent-home");
    if let Some(git) = git_link {
        command.arg("--ro-bind");
        command.arg(&git.common_dir).arg("/git-common");
        command
            .arg("--ro-bind")
            .arg(&git.file)
            .arg("/workspace/.git");
    }
    let client = work_dir.join("client");
    if client.is_dir() {
        command
            .arg("--ro-bind")
            .arg(client)
            .arg("/workspace/client");
    }
    command.args([
        "--tmpfs",
        "/home",
        "--dir",
        &format!("/home/{}", cfg.execution_user),
        "--dir",
        &format!("/home/{}/.cargo", cfg.execution_user),
        "--dir",
        &format!("/home/{}/.local", cfg.execution_user),
        "--ro-bind",
        &format!("/home/{}/.cargo/bin", cfg.execution_user),
        &format!("/home/{}/.cargo/bin", cfg.execution_user),
        "--ro-bind",
        &format!("/home/{}/.rustup", cfg.execution_user),
        &format!("/home/{}/.rustup", cfg.execution_user),
        "--ro-bind",
        &format!("/home/{}/.local/bin", cfg.execution_user),
        &format!("/home/{}/.local/bin", cfg.execution_user),
        "--tmpfs",
        "/opt/hank",
        "--tmpfs",
        "/opt/hank-src",
        "--tmpfs",
        "/opt/hank-worktrees",
        "--tmpfs",
        "/opt/hank-workspaces",
        "--tmpfs",
        "/opt/hank-agent-state",
        "--chdir",
        "/workspace",
    ]);
    command.arg(executable);

    match backend {
        "codex" => {
            if let Some(base_url) = auth.base_url.as_deref() {
                for value in codex_provider_overrides(base_url)? {
                    command.args(["-c", &value]);
                }
            }
            command.args([
                "-c",
                r#"shell_environment_policy.exclude=["OPENAI_API_KEY","ANTHROPIC_API_KEY","ANTHROPIC_AUTH_TOKEN","CLAUDE_CODE_OAUTH_TOKEN"]"#,
            ]);
            command.args(["exec"]);
            if let Some(thread_id) = previous_thread {
                command.args(["resume", "--json", "--ignore-user-config"]);
                if let Some(model) = auth.model.as_deref() {
                    command.args(["--model", model]);
                }
                command.args([
                    "--dangerously-bypass-approvals-and-sandbox",
                    "--skip-git-repo-check",
                ]);
                command.arg(thread_id).arg("-");
            } else {
                command.args([
                    "--json",
                    "--ignore-user-config",
                    "--color",
                    "never",
                    "--dangerously-bypass-approvals-and-sandbox",
                    "--skip-git-repo-check",
                    "-C",
                    "/workspace",
                ]);
                if let Some(model) = auth.model.as_deref() {
                    command.args(["--model", model]);
                }
                command.arg("-");
            }
        }
        "claude" => {
            command.args([
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--dangerously-skip-permissions",
                "--disable-slash-commands",
                "--strict-mcp-config",
                "--mcp-config",
                "{\"mcpServers\":{}}",
            ]);
            if let Some(thread_id) = previous_thread {
                command.args(["--resume", thread_id]);
            } else {
                command.args([
                    "--session-id",
                    state_dir
                        .file_name()
                        .and_then(|v| v.to_str())
                        .unwrap_or_default(),
                ]);
            }
            if let Some(model) = auth.model.as_deref() {
                command.args(["--model", model]);
            }
        }
        _ => unreachable!(),
    }
    #[cfg(unix)]
    command.as_std_mut().process_group(0);
    Ok(CliCommand {
        command,
        launcher_environment,
    })
}

/// 凭据经 stdin 前导协议传给切换用户后的隐藏启动器，不能进入 sudo 环境或参数日志。
fn sandbox_launcher_command(execution_user: &str, sandbox_bin: &str) -> Command {
    let mut command = Command::new("/usr/bin/sudo");
    command.env_clear();
    command.args([
        "-n",
        "-u",
        execution_user,
        SANDBOX_LAUNCHER_BIN,
        SANDBOX_LAUNCHER_ARG,
        sandbox_bin,
    ]);
    command
}

fn codex_provider_overrides(base_url: &str) -> Result<Vec<String>> {
    let base_url = serde_json::to_string(base_url).context("编码 Codex base URL")?;
    Ok(vec![
        r#"model_provider="trace_cli""#.to_string(),
        r#"model_providers.trace_cli.name="trace-cli""#.to_string(),
        format!("model_providers.trace_cli.base_url={base_url}"),
        r#"model_providers.trace_cli.wire_api="responses""#.to_string(),
        "model_providers.trace_cli.requires_openai_auth=true".to_string(),
    ])
}

async fn handle_json_line(
    state: &Arc<AppState>,
    session_id: &str,
    backend: &str,
    auth: &CliAuth,
    line: &str,
    run: &mut CliRunState,
) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        tracing::warn!(
            session_id,
            backend,
            "ignored malformed external agent JSONL"
        );
        return;
    };
    match backend {
        "codex" => handle_codex_event(state, session_id, auth, &value, run).await,
        "claude" | "grok" => handle_claude_event(state, session_id, auth, &value, run).await,
        "kimi" => handle_generic_event(state, session_id, auth, &value, run).await,
        _ => {}
    }
}

async fn handle_generic_event(
    state: &Arc<AppState>,
    session_id: &str,
    auth: &CliAuth,
    value: &serde_json::Value,
    run: &mut CliRunState,
) {
    if let Some(thread_id) = value["session_id"]
        .as_str()
        .or_else(|| value["thread_id"].as_str())
        .or_else(|| value["message"]["session_id"].as_str())
    {
        if run.thread_id.as_deref() != Some(thread_id) {
            run.thread_id = Some(thread_id.to_string());
            let _ = persist_thread_id(state, session_id, thread_id).await;
        }
    }
    if run.model.is_none() {
        run.model = value["model"]
            .as_str()
            .or_else(|| value["message"]["model"].as_str())
            .map(ToOwned::to_owned);
    }
    collect_paths(value, &mut run.changed_files);

    let event_type = value["type"].as_str().unwrap_or_default();
    if matches!(event_type, "result" | "final" | "agent_result") {
        if let Some(text) = value["result"]
            .as_str()
            .or_else(|| value["text"].as_str())
            .or_else(|| value["content"].as_str())
        {
            run.final_text = text.to_string();
        }
    }
    if matches!(event_type, "assistant" | "message" | "assistant_message") {
        let text = content_text_from_json(&value["message"]["content"])
            .or_else(|| content_text_from_json(&value["content"]));
        if let Some(text) = text.filter(|text| !text.trim().is_empty()) {
            run.final_text = text;
        }
    }
    if matches!(event_type, "content_block_delta" | "stream_event") {
        if let Some(text) = value["delta"]["text"]
            .as_str()
            .or_else(|| value["event"]["delta"]["text"].as_str())
            .or_else(|| value["text"].as_str())
        {
            run.final_text.push_str(text);
        }
    }

    let tool = if event_type == "content_block_start" {
        Some(&value["content_block"])
    } else if value["tool_use"].is_object() {
        Some(&value["tool_use"])
    } else {
        None
    };
    if let Some(tool) = tool.filter(|tool| tool["type"].as_str() == Some("tool_use")) {
        emit(
            state,
            session_id,
            AgentEvent::ToolStart {
                id: tool["id"].as_str().unwrap_or("remote-tool").to_string(),
                name: tool["name"].as_str().unwrap_or("remote_tool").to_string(),
                input: truncate(&redact_secrets(&tool["input"].to_string(), auth), 2000),
                run_id: None,
                turn_id: None,
                call_id: None,
                risk: None,
                timeout_ms: None,
            },
        )
        .await;
    }

    let usage = if value["usage"].is_object() {
        &value["usage"]
    } else {
        &value["message"]["usage"]
    };
    run.input_tokens = run.input_tokens.max(as_u32(&usage["input_tokens"]));
    run.output_tokens = run.output_tokens.max(as_u32(&usage["output_tokens"]));
    if event_type.contains("error") || value["is_error"].as_bool() == Some(true) {
        run.failed_message = value["error"]["message"]
            .as_str()
            .or_else(|| value["message"].as_str())
            .or_else(|| value["result"].as_str())
            .map(ToOwned::to_owned);
    }
}

fn content_text_from_json(value: &serde_json::Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    let blocks = value.as_array()?;
    let text = blocks
        .iter()
        .filter_map(|block| {
            if block["type"].as_str().is_some_and(|kind| kind == "text") {
                block["text"].as_str()
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("");
    (!text.is_empty()).then_some(text)
}

fn extract_final_text(output: &str) -> String {
    let mut complete = String::new();
    let mut deltas = String::new();
    let mut plain = String::new();
    for line in output.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            if !line.trim().is_empty() {
                plain = line.trim().to_string();
            }
            continue;
        };
        if let Some(text) = value["result"].as_str().or_else(|| value["final"].as_str()) {
            complete = text.to_string();
        }
        if let Some(text) = content_text_from_json(&value["message"]["content"])
            .or_else(|| content_text_from_json(&value["content"]))
        {
            complete = text;
        }
        if let Some(text) = value["delta"]["text"]
            .as_str()
            .or_else(|| value["event"]["delta"]["text"].as_str())
        {
            deltas.push_str(text);
        }
    }
    if complete.trim().is_empty() {
        if deltas.trim().is_empty() {
            plain
        } else {
            deltas
        }
    } else {
        complete
    }
}

async fn handle_codex_event(
    state: &Arc<AppState>,
    session_id: &str,
    auth: &CliAuth,
    value: &serde_json::Value,
    run: &mut CliRunState,
) {
    match value["type"].as_str().unwrap_or_default() {
        "thread.started" => {
            if let Some(thread_id) = value["thread_id"].as_str() {
                run.thread_id = Some(thread_id.to_string());
                let _ = persist_thread_id(state, session_id, thread_id).await;
            }
            // codex exec --json 的 thread.started 只有 thread_id，不自报模型（已实测确认），
            // 所以 codex 的模型名只能取 auth.model（来自 agent-cli.env 的 OPENAI_MODEL）。
        }
        "item.started" => {
            let item = &value["item"];
            if matches!(
                item["type"].as_str(),
                Some("command_execution" | "mcp_tool_call")
            ) {
                emit(
                    state,
                    session_id,
                    AgentEvent::ToolStart {
                        id: item["id"].as_str().unwrap_or("codex-tool").to_string(),
                        name: item["type"].as_str().unwrap_or("codex_tool").to_string(),
                        input: redact_secrets(item["command"].as_str().unwrap_or_default(), auth),
                        run_id: None,
                        turn_id: None,
                        call_id: None,
                        risk: None,
                        timeout_ms: None,
                    },
                )
                .await;
            }
        }
        "item.completed" => {
            let item = &value["item"];
            match item["type"].as_str().unwrap_or_default() {
                "agent_message" => {
                    if let Some(text) = item["text"].as_str() {
                        run.final_text = text.to_string();
                    }
                }
                "file_change" => collect_paths(item, &mut run.changed_files),
                "command_execution" | "mcp_tool_call" => {
                    emit(
                        state,
                        session_id,
                        AgentEvent::ToolResult {
                            id: item["id"].as_str().unwrap_or("codex-tool").to_string(),
                            name: item["type"].as_str().map(ToOwned::to_owned),
                            content: truncate(
                                &redact_secrets(
                                    item["aggregated_output"].as_str().unwrap_or_default(),
                                    auth,
                                ),
                                4000,
                            ),
                            is_error: item["status"]
                                .as_str()
                                .is_some_and(|status| status == "failed"),
                            run_id: None,
                            turn_id: None,
                            call_id: None,
                            duration_ms: None,
                        },
                    )
                    .await;
                }
                _ => {}
            }
        }
        "turn.completed" => {
            run.input_tokens = as_u32(&value["usage"]["input_tokens"]);
            run.output_tokens = as_u32(&value["usage"]["output_tokens"]);
        }
        "turn.failed" => {
            run.failed_message = value["error"]["message"]
                .as_str()
                .or_else(|| value["message"].as_str())
                .map(ToOwned::to_owned);
        }
        _ => {}
    }
}

async fn handle_claude_event(
    state: &Arc<AppState>,
    session_id: &str,
    auth: &CliAuth,
    value: &serde_json::Value,
    run: &mut CliRunState,
) {
    match value["type"].as_str().unwrap_or_default() {
        "system" => {
            if let Some(thread_id) = value["session_id"].as_str() {
                run.thread_id = Some(thread_id.to_string());
                let _ = persist_thread_id(state, session_id, thread_id).await;
            }
            // claude --output-format stream-json 的 init 事件带实际模型名。
            if let Some(model) = value["model"].as_str() {
                run.model = Some(model.to_string());
            }
        }
        "assistant" => {
            if run.model.is_none() {
                if let Some(model) = value["message"]["model"].as_str() {
                    run.model = Some(model.to_string());
                }
            }
            if let Some(blocks) = value["message"]["content"].as_array() {
                for block in blocks {
                    if block["type"].as_str() == Some("tool_use") {
                        let tool_name = block["name"].as_str().unwrap_or("claude_tool");
                        if matches!(tool_name, "Write" | "Edit" | "NotebookEdit") {
                            if let Some(path) = block["input"]["file_path"].as_str() {
                                run.changed_files.insert(path.to_string());
                            }
                        }
                        emit(
                            state,
                            session_id,
                            AgentEvent::ToolStart {
                                id: block["id"].as_str().unwrap_or("claude-tool").to_string(),
                                name: tool_name.to_string(),
                                input: truncate(
                                    &redact_secrets(&block["input"].to_string(), auth),
                                    2000,
                                ),
                                run_id: None,
                                turn_id: None,
                                call_id: None,
                                risk: None,
                                timeout_ms: None,
                            },
                        )
                        .await;
                    }
                }
            }
        }
        "user" => {
            if let Some(blocks) = value["message"]["content"].as_array() {
                for block in blocks {
                    if block["type"].as_str() != Some("tool_result") {
                        continue;
                    }
                    let content = block["content"]
                        .as_str()
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| block["content"].to_string());
                    emit(
                        state,
                        session_id,
                        AgentEvent::ToolResult {
                            id: block["tool_use_id"]
                                .as_str()
                                .unwrap_or("claude-tool")
                                .to_string(),
                            name: None,
                            content: truncate(&redact_secrets(&content, auth), 4000),
                            is_error: block["is_error"].as_bool().unwrap_or(false),
                            run_id: None,
                            turn_id: None,
                            call_id: None,
                            duration_ms: None,
                        },
                    )
                    .await;
                }
            }
        }
        "result" => {
            if let Some(text) = value["result"].as_str() {
                run.final_text = text.to_string();
            }
            run.input_tokens = as_u32(&value["usage"]["input_tokens"]);
            run.output_tokens = as_u32(&value["usage"]["output_tokens"]);
            if value["is_error"].as_bool().unwrap_or(false) {
                run.failed_message = value["result"].as_str().map(ToOwned::to_owned);
            }
        }
        _ => {}
    }
}

async fn resolve_cli_auth(state: &Arc<AppState>, backend: &str) -> Result<CliAuth> {
    // 优先用 admin 在库里启用的那份配置，切换后下一轮任务即生效，不必重启 systemd。
    // 没有启用行或该行没填 key 时继续往下用环境文件兜底。
    if let Ok(Some(config)) = state.db.get_active_agent_cli_profile(backend).await {
        if let Some(auth) = auth_from_db_config(backend, &config) {
            return Ok(auth);
        }
    }

    let mut auth = CliAuth::default();
    let relevant_keys: &[&str] = match backend {
        "codex" => &["OPENAI_API_KEY", "OPENAI_BASE_URL", "OPENAI_MODEL"],
        "claude" => &[
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
        ],
        _ => bail!("不支持的外部 Agent 后端: {backend}"),
    };
    for key in relevant_keys {
        if let Ok(value) = std::env::var(key) {
            if !value.is_empty() {
                if *key == "OPENAI_BASE_URL" {
                    // Codex 第三方端点通过命令行组装完整 custom Responses provider。
                    auth.base_url = Some(value);
                } else if matches!(*key, "OPENAI_MODEL" | "ANTHROPIC_MODEL") {
                    auth.model = Some(value.clone());
                    if *key == "ANTHROPIC_MODEL" {
                        auth.env.push((key, value));
                    }
                } else {
                    auth.env.push((key, value));
                }
            }
        }
    }
    let has_runtime_key = match backend {
        "codex" => auth.env.iter().any(|(key, _)| *key == "OPENAI_API_KEY"),
        "claude" => auth.env.iter().any(|(key, _)| {
            matches!(
                *key,
                "ANTHROPIC_API_KEY" | "ANTHROPIC_AUTH_TOKEN" | "CLAUDE_CODE_OAUTH_TOKEN"
            )
        }),
        _ => false,
    };
    if has_runtime_key {
        return Ok(auth);
    }

    let providers = state.db.list_providers_ordered().await?;
    let provider = providers.into_iter().find(|provider| {
        provider.enabled
            && match backend {
                "claude" => provider.provider_type == "anthropic",
                "codex" => codex_provider_is_compatible(provider),
                _ => false,
            }
    });
    let provider = provider.ok_or_else(|| match backend {
        "codex" => anyhow!(
            "codex 没有 Responses API 凭据：请在 admin 的「Agent CLI」页配置，或在 /opt/hank/agent-cli.env 配置 OPENAI_API_KEY 与兼容 Responses API 的 OPENAI_BASE_URL"
        ),
        _ => anyhow!(
            "{backend} 没有可用凭据：请在 admin 的「Agent CLI」页配置，或配置 /opt/hank/agent-cli.env 或启用对应 provider"
        ),
    })?;
    if provider.api_key.trim().is_empty() {
        bail!("{backend} provider 的 API key 为空");
    }
    auth.model = Some(crate::provider_registry::resolve_default_model(&provider));
    match backend {
        "codex" => {
            auth.env.push(("OPENAI_API_KEY", provider.api_key));
            if !provider.base_url.trim().is_empty() {
                auth.base_url = Some(provider.base_url);
            }
        }
        "claude" => {
            auth.env.push(("ANTHROPIC_API_KEY", provider.api_key));
            if !provider.base_url.trim().is_empty() {
                auth.env.push(("ANTHROPIC_BASE_URL", provider.base_url));
            }
        }
        _ => bail!("不支持的外部 Agent 后端: {backend}"),
    }
    Ok(auth)
}

/// 外部 Agent 后端的固定优先级（新话题默认选择时使用）。
const PREFERRED_EXTERNAL_BACKEND_ORDER: [&str; 4] = ["codex", "claude", "grok", "kimi"];

/// 从「当前用户在线节点已上报的 backend 能力」中按固定优先级选默认外部 Agent。
/// 只认 codex/claude/grok/kimi；未知值忽略。无可用能力时返回 None，由调用方走明确失败路径。
///
/// 不读 server DB/env/provider 凭据：client-only 路径能否执行取决于本机 hank-cli 节点能力。
pub(crate) fn preferred_backend_from_online_capabilities<'a>(
    available: impl IntoIterator<Item = &'a str>,
) -> Option<&'static str> {
    let available: std::collections::HashSet<&str> = available
        .into_iter()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect();
    PREFERRED_EXTERNAL_BACKEND_ORDER
        .into_iter()
        .find(|backend| available.contains(backend))
}

/// 新话题未明确指定 CLI 时，优先选择当前用户在线 hank-cli 节点实际具备的后端。
/// 无在线能力时返回 "codex" 作为确定失败路径（create_feishu_session 会报缺少对应节点）。
pub(crate) async fn preferred_backend(state: &AppState, user_id: &str) -> &'static str {
    // 探测在线节点实际具备的 backend；pick_online_agent_client 自身读 hub 读锁，线程安全。
    let mut available = Vec::new();
    for backend in PREFERRED_EXTERNAL_BACKEND_ORDER {
        if crate::remote_exec::pick_online_agent_client(state, user_id, backend)
            .await
            .is_some()
        {
            available.push(backend);
        }
    }
    // 统一走 helper：codex → claude → grok → kimi；无能力时仍回落 codex 明确失败路径。
    preferred_backend_from_online_capabilities(available).unwrap_or("codex")
}

/// 计算某后端当前实际生效的凭据来源，供 admin 页展示「库里的配置是否真的在用」。
/// 与 resolve_cli_auth 的优先级保持一致：库 → 环境文件 → provider 记录。
pub(crate) async fn effective_auth_source(
    state: &Arc<AppState>,
    backend: &str,
) -> Option<AuthSource> {
    if let Ok(Some(config)) = state.db.get_active_agent_cli_profile(backend).await {
        if auth_from_db_config(backend, &config).is_some() {
            return Some(AuthSource::Db);
        }
    }
    let (auth_kinds, _) = backend_env_whitelist(backend)?;
    if auth_kinds
        .iter()
        .any(|key| std::env::var(key).is_ok_and(|value| !value.trim().is_empty()))
    {
        return Some(AuthSource::Env);
    }
    let providers = state.db.list_providers_ordered().await.unwrap_or_default();
    let has_provider = providers.iter().any(|provider| match backend {
        "claude" => provider.enabled && provider.provider_type == "anthropic",
        "codex" => codex_provider_is_compatible(provider),
        _ => false,
    });
    has_provider.then_some(AuthSource::Provider)
}

fn codex_provider_is_compatible(provider: &hank_db::ProviderRecord) -> bool {
    if !provider.enabled || provider.provider_type != "openai" {
        return false;
    }
    let base_url = provider.base_url.trim().trim_end_matches('/');
    base_url.is_empty()
        || matches!(
            base_url,
            "https://api.openai.com" | "https://api.openai.com/v1"
        )
}

async fn validate_runtime(state: &Arc<AppState>, backend: &str) -> Result<()> {
    let cfg = &state.config.server_agent;
    let executable = Path::new(&cfg.agent_cli_root)
        .join(backend)
        .join("current/bin")
        .join(if backend == "claude" {
            "claude"
        } else {
            "codex"
        });
    if !executable.is_file() {
        bail!("{backend} 尚未离线安装: {}", executable.display());
    }
    if !Path::new(&cfg.agent_sandbox_bin).is_file() {
        bail!("外部 Agent 文件沙箱未安装: {}", cfg.agent_sandbox_bin);
    }
    if !Path::new(SANDBOX_LAUNCHER_BIN).is_file() {
        bail!("外部 Agent 安全启动器不可用: {SANDBOX_LAUNCHER_BIN}");
    }
    for mountpoint in ["/workspace", "/agent-home", "/git-common"] {
        if !Path::new(mountpoint).is_dir() {
            bail!("外部 Agent 沙箱挂载点不存在: {mountpoint}");
        }
    }
    Ok(())
}

fn validate_workspace(state: &Arc<AppState>, path: &Path) -> Result<()> {
    let cfg = &state.config.server_agent;
    let allowed = [&cfg.worktrees_root, &cfg.general_workspaces_root]
        .iter()
        .filter_map(|root| std::fs::canonicalize(root).ok())
        .any(|root| path.starts_with(&root) && path != root);
    if !allowed {
        bail!("工作目录不在 server_agent 允许的根目录内");
    }
    Ok(())
}

async fn prepare_state_dir(state: &Arc<AppState>, session_id: &str) -> Result<PathBuf> {
    let root = Path::new(&state.config.server_agent.agent_state_root);
    let canonical_root = tokio::fs::canonicalize(root)
        .await
        .context("agent_state_root 不存在，请先运行离线安装脚本")?;
    let dir = canonical_root.join(session_id);
    tokio::fs::create_dir_all(&dir).await?;
    ensure_real_dir(&dir.join(".codex")).await?;
    ensure_real_dir(&dir.join(".claude")).await?;
    remove_state_file(&dir.join(".gitconfig")).await?;
    tokio::fs::write(dir.join(".gitconfig"), "[safe]\n\tdirectory = /workspace\n").await?;
    #[cfg(unix)]
    for path in [&dir, &dir.join(".codex"), &dir.join(".claude")] {
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o770)).await?;
    }
    let canonical = tokio::fs::canonicalize(&dir).await?;
    if !canonical.starts_with(&canonical_root) || canonical == canonical_root {
        bail!("Agent 状态目录越界");
    }
    Ok(canonical)
}

async fn prepare_git_link(
    state: &Arc<AppState>,
    work_dir: &Path,
    state_dir: &Path,
) -> Result<Option<GitLink>> {
    let dot_git = work_dir.join(".git");
    if !dot_git.is_file() {
        return Ok(None);
    }
    let raw = tokio::fs::read_to_string(&dot_git).await?;
    let git_dir = raw
        .trim()
        .strip_prefix("gitdir:")
        .map(str::trim)
        .ok_or_else(|| anyhow!("worktree .git 格式无效"))?;
    let git_dir = tokio::fs::canonicalize(git_dir).await?;
    let common_dir = git_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| anyhow!("无法定位 Git common dir"))?
        .to_path_buf();
    let expected_common_dir =
        tokio::fs::canonicalize(Path::new(&state.config.server_agent.repository_root).join(".git"))
            .await
            .context("无法定位 server_agent Git common dir")?;
    if common_dir != expected_common_dir
        || !git_dir.starts_with(expected_common_dir.join("worktrees"))
    {
        bail!("worktree Git 元数据不属于 server_agent 仓库");
    }
    let worktree_name = git_dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow!("Git worktree 名称无效"))?;
    let file = state_dir.join("gitlink");
    remove_state_file(&file).await?;
    tokio::fs::write(
        &file,
        format!("gitdir: /git-common/worktrees/{worktree_name}\n"),
    )
    .await?;
    #[cfg(unix)]
    tokio::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o440)).await?;
    Ok(Some(GitLink { file, common_dir }))
}

async fn ensure_real_dir(path: &Path) -> Result<()> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("Agent 状态子目录不能是符号链接: {}", path.display())
        }
        Ok(metadata) if !metadata.is_dir() => {
            bail!("Agent 状态路径不是目录: {}", path.display())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tokio::fs::create_dir(path).await?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

async fn remove_state_file(path: &Path) -> Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn content_text(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn agent_prompt(user_text: &str, agent_kind: &str) -> String {
    let mut prompt = user_text.to_string();
    prompt.push_str("\n\n运行约束：只操作当前 /workspace；client/ 只读且不属于任务范围；不要读取或修改 config.toml。完成后用中文给出结果和验证情况。");
    if agent_kind == "quant_code" {
        prompt.push_str(
            "修改 quant 前必须读取 /workspace/quant/AGENTS.md，并遵守禁止交易能力的产品边界。",
        );
    }
    prompt
}

fn local_agent_prompt(user_text: &str, agent_kind: &str) -> String {
    let mut prompt = user_text.to_string();
    prompt.push_str("\n\n运行约束：只操作 hank-cli 提供的当前工作目录及其子目录；遵循目录中的 AGENTS.md/CLAUDE.md 等项目规则；不要读取或修改凭据、密钥或本机 Agent 认证配置。完成后用中文给出结果和验证情况。");
    if agent_kind == "quant_code" {
        prompt.push_str("修改 quant 前必须读取 quant/AGENTS.md，并遵守禁止交易能力的产品边界。");
    }
    prompt
}

/// 是否对本轮启用两阶段闸门。抽成纯函数：判定条件多，且必须可单测。
///
/// 注意：`task_gate_enabled` 与 `server_agent.enabled` 解耦——后者关闭时
/// client-only 链路仍可开闸门，调用方不要再加 `&& cfg.enabled`。
fn should_gate_turn(
    task_gate_enabled: bool,
    agent_kind: &str,
    source: Option<&str>,
    existing_thread_id: Option<&str>,
) -> bool {
    if !task_gate_enabled {
        return false;
    }
    // conversation 无工具、quant_research 是 REST 研究，都不走代码闸门。
    if !matches!(agent_kind, "trace_code" | "quant_code" | "general_task") {
        return false;
    }
    // 微信没有按钮卡片，文本确认长分析体验差，本次只做飞书。
    if source != Some("feishu") {
        return false;
    }
    // 已有 thread 说明是续聊，续聊不再弹闸门。
    if existing_thread_id.is_some_and(|id| !id.is_empty()) {
        return false;
    }
    true
}

/// 闸门第一轮 prompt：只读不改，产出结构化分析。
///
/// 注意 CLI 是以 bypass-approvals 启动的，写操作不会被沙箱拦住——这里只能靠
/// 指令约束，配合第二轮前的 git status 事后检查。所以措辞要强，且要求它
/// 在无法只读完成时明说，而不是擅自动手。
fn local_agent_analysis_prompt(user_text: &str, agent_kind: &str) -> String {
    let mut prompt = String::new();
    prompt.push_str("【本轮任务：只读分析，不要改代码】\n\n");
    prompt.push_str(user_text);
    prompt.push_str(
        "\n\n运行约束（必须严格遵守）：\n\
         - 本轮**只读**：可以读文件、搜索、看 git log / diff，**不要**修改、创建、删除任何文件。\n\
         - **不要**执行会改变状态的命令（安装依赖、格式化、提交、checkout、reset 等）。\n\
         - 只操作 hank-cli 提供的当前工作目录及其子目录；不要读取或修改凭据、密钥或本机 Agent 认证配置。\n\
         - 输出固定四段 markdown，不要额外前言/后记：\n\
         ## 目标\n\
         ## 范围\n\
         ## 疑似改动点\n\
         ## 风险\n\
         - 结尾**不要**问「要我开始吗」——是否开始由用户在飞书卡片上点按钮决定。\n\
         - 若任务无法在只读前提下分析清楚，在「## 风险」里说明缺什么，**仍然不要动手**。",
    );
    if agent_kind == "quant_code" {
        prompt.push_str(
            "\n- 分析 quant 相关改动前必须读取 quant/AGENTS.md，并遵守禁止交易能力的产品边界。",
        );
    }
    prompt
}

/// 第一轮分析成功后落 task_gate 交互单并发 AskUser，让 pusher 停在「等待确认」。
#[allow(clippy::too_many_arguments)]
async fn finish_as_task_gate(
    state: &Arc<AppState>,
    session_id: &str,
    user_id: &str,
    client_id: &str,
    backend: &str,
    agent_kind: &str,
    user_text: &str,
    analysis: &str,
    thread_id: &str,
) -> Result<()> {
    let dirty_files = count_dirty_files_via_remote(state, user_id, client_id).await;
    let feishu_chat = state
        .db
        .get_feishu_chat_by_session(session_id)
        .await
        .context("反查 feishu_chats")?;
    let (account_id, chat_id, topic_id) = match feishu_chat.as_ref() {
        Some(c) => (
            Some(c.account_id.as_str()),
            Some(c.chat_id.as_str()),
            Some(c.topic_id.as_str()),
        ),
        None => {
            tracing::warn!(
                session_id,
                "task_gate：session 无 feishu_chats 映射，交互单 account/chat/topic 为空"
            );
            (None, None, None)
        }
    };

    let goal: String = user_text.chars().take(2000).collect();
    let options_json = serde_json::to_string(&["开始修", "跳过"])
        .unwrap_or_else(|_| r#"["开始修","跳过"]"#.to_string());
    let resume_ref = serde_json::json!({
        "backend": backend,
        "thread_id": thread_id,
        "exec_client_id": client_id,
        "agent_kind": agent_kind,
        "dirty_files": dirty_files,
    })
    .to_string();

    let row = state
        .db
        .create_interaction(NewInteraction {
            session_id,
            user_id,
            channel: "feishu",
            account_id,
            chat_id,
            topic_id,
            kind: "task_gate",
            title: "新任务 · 待确认是否开始修",
            goal: Some(&goal),
            analysis: Some(analysis),
            options: &options_json,
            resume_ref: Some(&resume_ref),
            expires_at: None,
        })
        .await
        .context("创建 task_gate 交互单")?;

    tracing::info!(
        interaction_id = %row.id,
        session_id,
        dirty_files,
        "task_gate 交互单已落表，等待用户确认是否开始修"
    );

    emit(
        state,
        session_id,
        AgentEvent::AskUser {
            question: goal,
            options: vec!["开始修".to_string(), "跳过".to_string()],
            tool_use_id: format!("task_gate:{}", row.id),
            kind: Some("task_gate".to_string()),
        },
    )
    .await;
    Ok(())
}

/// 第二轮结束后把 interaction_flow 挂在 session metadata 上的 active_task_gate_id 标终态。
///
/// 不用 latest_pending_interaction：派发后状态是 executing，且 pending 查询会误伤
/// 第一轮刚落、尚未应答的闸门单。
async fn finalize_open_task_gate(
    state: &Arc<AppState>,
    session_id: &str,
    status: &str,
    result: Option<&str>,
    error: Option<&str>,
) {
    let Ok(Some(session)) = state.db.get_session(session_id).await else {
        return;
    };
    let mut metadata = parse_metadata(session.metadata.as_deref());
    let Some(interaction_id) = metadata
        .get("active_task_gate_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
    else {
        return;
    };
    if let Err(e) = state
        .db
        .update_interaction_status(&interaction_id, status, result, error)
        .await
    {
        tracing::warn!(
            interaction_id = %interaction_id,
            session_id,
            "finalize task_gate 失败: {e:#}"
        );
    }
    metadata
        .as_object_mut()
        .map(|m| m.remove("active_task_gate_id"));
    if let Err(e) = state
        .db
        .update_session_metadata(session_id, &metadata.to_string())
        .await
    {
        tracing::warn!(session_id, "清除 active_task_gate_id 失败: {e:#}");
    }
}

/// 派发第二轮前把交互单 id 挂到 session，供 cli_agent 终态回调 finalize。
pub async fn attach_active_task_gate(
    state: &Arc<AppState>,
    session_id: &str,
    interaction_id: &str,
) -> Result<()> {
    let session = state
        .db
        .get_session(session_id)
        .await?
        .ok_or_else(|| anyhow!("会话不存在"))?;
    let mut metadata = parse_metadata(session.metadata.as_deref());
    metadata["active_task_gate_id"] = serde_json::Value::String(interaction_id.to_string());
    state
        .db
        .update_session_metadata(session_id, &metadata.to_string())
        .await?;
    Ok(())
}

/// 派发失败时清掉 active_task_gate_id，避免误标后续无关轮次。
pub async fn clear_active_task_gate(state: &Arc<AppState>, session_id: &str) -> Result<()> {
    let session = state
        .db
        .get_session(session_id)
        .await?
        .ok_or_else(|| anyhow!("会话不存在"))?;
    let mut metadata = parse_metadata(session.metadata.as_deref());
    if metadata
        .as_object_mut()
        .is_some_and(|m| m.remove("active_task_gate_id").is_some())
    {
        state
            .db
            .update_session_metadata(session_id, &metadata.to_string())
            .await?;
    }
    Ok(())
}

/// 事后检查：本机节点 git status --porcelain 行数。失败/非 git 目录返回 0，不阻塞闸门。
async fn count_dirty_files_via_remote(
    state: &Arc<AppState>,
    user_id: &str,
    client_id: &str,
) -> usize {
    if !crate::remote_exec::is_client_online(state, user_id, client_id).await {
        return 0;
    }
    match crate::remote_exec::dispatch_tool_call(
        state,
        user_id,
        client_id,
        "shell",
        serde_json::json!({ "command": "git status --porcelain" }),
        Duration::from_secs(30),
    )
    .await
    {
        Ok(result) if !result.is_error => result
            .content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count(),
        Ok(_) | Err(_) => 0,
    }
}

/// client-only 会话 RunStarted 展示 cwd：只用绑定节点注册的 work_dir。
/// 缺失或空白时返回非路径占位文本；调用方不得传入 session.work_dir 作为 fallback
///（session 可能残留 wananyun/server worktree 绝对路径）。
fn client_only_display_cwd(client_work_dir: Option<&str>) -> String {
    client_work_dir
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "(hank-cli work_dir)".to_string())
}

fn parse_metadata(raw: Option<&str>) -> serde_json::Value {
    raw.and_then(|value| serde_json::from_str(value).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

async fn persist_thread_id(state: &Arc<AppState>, session_id: &str, thread_id: &str) -> Result<()> {
    let session = state
        .db
        .get_session(session_id)
        .await?
        .ok_or_else(|| anyhow!("会话不存在"))?;
    let mut metadata = parse_metadata(session.metadata.as_deref());
    metadata["agent_thread_id"] = serde_json::Value::String(thread_id.to_string());
    state
        .db
        .update_session_metadata(session_id, &metadata.to_string())
        .await?;
    Ok(())
}

async fn emit(state: &Arc<AppState>, session_id: &str, event: AgentEvent) {
    let mut buffers = state.event_buffers.write().await;
    if let Some(buffer) = buffers.get_mut(session_id) {
        buffer.push(event);
    }
}

/// 输出超限时保住已产出的内容：把 partial final_text 落库成 assistant 消息，
/// 再报失败。整轮丢弃会让用户白等一次长任务。
async fn persist_partial_before_failure(
    state: &Arc<AppState>,
    session_id: &str,
    parent_id: &str,
    partial: &str,
) {
    let partial = partial.trim();
    if partial.is_empty() {
        return;
    }
    let content = serde_json::json!([{
        "type": "text",
        "text": format!("{partial}\n\n（输出超过安全上限，以上为截断前已产出内容）"),
    }]);
    match state
        .db
        .save_message(
            session_id,
            "assistant",
            &content,
            chrono::Utc::now(),
            Some(parent_id),
        )
        .await
    {
        Ok(assistant_id) => {
            if let Err(error) = state.db.update_active_leaf(session_id, &assistant_id).await {
                tracing::warn!(session_id, "更新超限 partial 消息游标失败: {error:#}");
            }
        }
        Err(error) => {
            tracing::warn!(session_id, "保存超限 partial 消息失败: {error:#}");
        }
    }
}

async fn emit_failed(state: &Arc<AppState>, session_id: &str, run_id: &str, message: &str) {
    emit(
        state,
        session_id,
        AgentEvent::RunFailed {
            run_id: run_id.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            message: message.to_string(),
        },
    )
    .await;
}

async fn read_limited<R: tokio::io::AsyncRead + Unpin>(reader: R, limit: usize) -> Result<String> {
    let mut lines = BufReader::new(reader).lines();
    let mut output = String::new();
    let mut truncated = false;
    while let Some(line) = lines.next_line().await? {
        if truncated {
            continue;
        }
        if output.len() + line.len() + 1 > limit {
            output.push_str("\n...[stderr truncated]");
            truncated = true;
            continue;
        }
        output.push_str(&line);
        output.push('\n');
    }
    Ok(output)
}

async fn terminate_process_group(child: &mut Child) {
    if let Some(pid) = child.id() {
        let group = format!("-{pid}");
        let _ = Command::new("kill").args(["-TERM", &group]).status().await;
        if tokio::time::timeout(Duration::from_secs(3), child.wait())
            .await
            .is_err()
        {
            let _ = Command::new("kill").args(["-KILL", &group]).status().await;
            let _ = child.wait().await;
        }
    }
}

async fn wait_after_output(child: &mut Child) -> Option<std::process::ExitStatus> {
    match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
        Ok(Ok(status)) => Some(status),
        _ => {
            terminate_process_group(child).await;
            None
        }
    }
}

fn collect_paths(item: &serde_json::Value, files: &mut HashSet<String>) {
    if let Some(changes) = item["changes"].as_array() {
        for change in changes {
            if let Some(path) = change["path"].as_str() {
                files.insert(path.to_string());
            }
        }
    }
}

fn file_changes(run: &CliRunState) -> Vec<FileChange> {
    run.changed_files
        .iter()
        .map(|path| FileChange {
            path: path.clone(),
            kind: FileChangeKind::Update,
        })
        .collect()
}

fn sanitize_error(stderr: &str, status: Option<&std::process::ExitStatus>) -> String {
    let text = stderr
        .lines()
        .filter(|line| !line.to_ascii_lowercase().contains("api_key"))
        .take(20)
        .collect::<Vec<_>>()
        .join("\n");
    if text.trim().is_empty() {
        format!(
            "外部 Agent 异常退出: {}",
            status
                .map(ToString::to_string)
                .unwrap_or_else(|| "unknown".to_string())
        )
    } else {
        truncate(&text, 4000)
    }
}

fn redact_secrets(text: &str, auth: &CliAuth) -> String {
    let mut redacted = text.to_string();
    for key in AUTH_ENV_KEYS {
        if !key.ends_with("_KEY") && !key.ends_with("_TOKEN") {
            continue;
        }
        if let Ok(value) = std::env::var(key) {
            if !value.is_empty() {
                redacted = redacted.replace(&value, "[redacted]");
            }
        }
    }
    for (key, value) in &auth.env {
        if matches!(
            *key,
            "OPENAI_API_KEY"
                | "ANTHROPIC_API_KEY"
                | "ANTHROPIC_AUTH_TOKEN"
                | "CLAUDE_CODE_OAUTH_TOKEN"
        ) && !value.is_empty()
        {
            redacted = redacted.replace(value, "[redacted]");
        }
    }
    redacted
}

fn truncate(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n...[truncated]", &text[..end])
}

fn as_u32(value: &serde_json::Value) -> u32 {
    value.as_u64().unwrap_or_default().min(u32::MAX as u64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn launcher_protocol_round_trips_without_consuming_prompt() {
        let environment = vec![
            ("HOME".to_string(), "/agent-home".to_string()),
            ("OPENAI_API_KEY".to_string(), "provider-secret".to_string()),
        ];
        let mut input = encode_launcher_environment(&environment).unwrap();
        input.extend_from_slice(b"prompt remains on stdin");

        let mut cursor = Cursor::new(input);
        let decoded = read_launcher_environment(&mut cursor).unwrap();
        let mut prompt = String::new();
        std::io::Read::read_to_string(&mut cursor, &mut prompt).unwrap();

        assert_eq!(decoded, environment);
        assert_eq!(prompt, "prompt remains on stdin");
    }

    #[test]
    fn launcher_protocol_rejects_unapproved_or_duplicate_environment() {
        let unapproved = vec![("LD_PRELOAD".to_string(), "/tmp/inject.so".to_string())];
        assert!(encode_launcher_environment(&unapproved).is_err());

        let duplicate = vec![
            ("OPENAI_API_KEY".to_string(), "first".to_string()),
            ("OPENAI_API_KEY".to_string(), "second".to_string()),
        ];
        assert!(encode_launcher_environment(&duplicate).is_err());
    }

    #[test]
    fn codex_relay_uses_explicit_responses_provider() {
        let overrides = codex_provider_overrides("https://relay.example.com/v1").unwrap();
        assert!(overrides
            .iter()
            .any(|value| value == r#"model_provider="trace_cli""#));
        assert!(overrides
            .iter()
            .any(|value| value == r#"model_providers.trace_cli.wire_api="responses""#));
        assert!(overrides
            .iter()
            .any(|value| value == "model_providers.trace_cli.requires_openai_auth=true"));
        assert!(overrides
            .iter()
            .any(|value| value.contains("https://relay.example.com/v1")));
        assert!(!overrides
            .iter()
            .any(|value| value.starts_with("openai_base_url=")));
    }

    #[test]
    fn local_agent_prompt_does_not_embed_server_workspace_paths() {
        let prompt = local_agent_prompt("修一下 bug", "trace_code");
        assert!(prompt.contains("hank-cli"));
        assert!(!prompt.contains("/opt/hank"));
        assert!(!prompt.contains("/workspace"));
    }

    #[test]
    fn should_gate_turn_hard_off_when_disabled() {
        // 无回归硬判定：开关关闭时恒为 false，路径与现在完全一致。
        assert!(!should_gate_turn(false, "trace_code", Some("feishu"), None));
        assert!(!should_gate_turn(false, "quant_code", Some("feishu"), None));
        assert!(!should_gate_turn(
            false,
            "general_task",
            Some("feishu"),
            None
        ));
    }

    #[test]
    fn should_gate_turn_requires_code_kind_feishu_and_fresh_thread() {
        assert!(should_gate_turn(true, "trace_code", Some("feishu"), None));
        assert!(should_gate_turn(true, "quant_code", Some("feishu"), None));
        assert!(should_gate_turn(true, "general_task", Some("feishu"), None));
        assert!(!should_gate_turn(
            true,
            "conversation",
            Some("feishu"),
            None
        ));
        assert!(!should_gate_turn(
            true,
            "quant_research",
            Some("feishu"),
            None
        ));
        assert!(!should_gate_turn(true, "trace_code", Some("weixin"), None));
        assert!(!should_gate_turn(true, "trace_code", None, None));
        assert!(!should_gate_turn(
            true,
            "trace_code",
            Some("feishu"),
            Some("thread-already-there")
        ));
    }

    #[test]
    fn local_agent_analysis_prompt_is_read_only_and_structured() {
        let prompt = local_agent_analysis_prompt("修一下 bug", "trace_code");
        assert!(prompt.contains("只读"));
        assert!(prompt.contains("不要修改") || prompt.contains("**不要**修改"));
        assert!(prompt.contains("## 目标"));
        assert!(prompt.contains("## 范围"));
        assert!(prompt.contains("## 疑似改动点"));
        assert!(prompt.contains("## 风险"));
        assert!(!prompt.contains("/opt/hank"));
        assert!(!prompt.contains("/workspace"));
    }

    #[test]
    fn preferred_backend_from_online_capabilities_follows_priority_order() {
        // 本机只有 claude 时，即使 server 有 codex 凭据，也应选 claude。
        assert_eq!(
            preferred_backend_from_online_capabilities(["claude"]),
            Some("claude")
        );
        assert_eq!(
            preferred_backend_from_online_capabilities(["kimi", "claude", "grok"]),
            Some("claude")
        );
        assert_eq!(
            preferred_backend_from_online_capabilities(["kimi", "grok"]),
            Some("grok")
        );
        assert_eq!(
            preferred_backend_from_online_capabilities(["codex", "claude"]),
            Some("codex")
        );
        assert_eq!(
            preferred_backend_from_online_capabilities(["kimi"]),
            Some("kimi")
        );
        assert_eq!(
            preferred_backend_from_online_capabilities(std::iter::empty::<&str>()),
            None
        );
        // 未知/非外部 backend 不参与选择；无有效能力时返回 None，走明确失败路径。
        assert_eq!(
            preferred_backend_from_online_capabilities(["native", "bash", ""]),
            None
        );
    }

    #[test]
    fn client_only_metadata_requires_remote_path() {
        let metadata = serde_json::json!({
            "agent_location": "client",
            "agent_backend": "codex",
            "exec_client_id": "node-1",
        });
        assert_eq!(metadata["agent_location"].as_str(), Some("client"));
        assert!(metadata["exec_client_id"].as_str().is_some());
    }

    #[test]
    fn client_only_display_cwd_never_falls_back_to_session_work_dir() {
        // 节点已注册本机目录时，展示该目录。
        assert_eq!(
            client_only_display_cwd(Some("/Users/me/code/trace")),
            "/Users/me/code/trace"
        );
        // 读取失败 / 缺失 / 空白：用明确非路径占位，不接受 session.work_dir 参数，
        // 从 API 上杜绝把 server/wananyun worktree 路径（如 /opt/hank/...）用于 RunStarted。
        assert_eq!(client_only_display_cwd(None), "(hank-cli work_dir)");
        assert_eq!(client_only_display_cwd(Some("")), "(hank-cli work_dir)");
        assert_eq!(client_only_display_cwd(Some("   ")), "(hank-cli work_dir)");
        let server_session_work_dir = Some("/opt/hank/worktrees/feishu-abc");
        // 即便 session 有 server 路径，display 解析也只看节点 work_dir；节点缺失时是占位而非 server 路径。
        assert_ne!(
            client_only_display_cwd(None),
            server_session_work_dir.unwrap()
        );
        assert!(!client_only_display_cwd(None).starts_with('/'));
    }

    #[test]
    fn sudo_command_contains_only_fixed_launcher_arguments() {
        let command = sandbox_launcher_command("hank-build", SANDBOX_BIN);
        let args = command
            .as_std()
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            vec![
                "-n",
                "-u",
                "hank-build",
                SANDBOX_LAUNCHER_BIN,
                SANDBOX_LAUNCHER_ARG,
                SANDBOX_BIN,
            ]
        );
        assert!(command.as_std().get_envs().next().is_none());
        assert!(!args.iter().any(|arg| arg.contains("preserve-env")));
    }

    /// 回写 sessions.model 时优先用 CLI 自报的模型名（claude 的 init 事件带 model），
    /// 拿不到时退回 agent-cli.env 里配置的模型（codex 只有这一个来源）。
    #[test]
    fn resolved_model_prefers_cli_reported_over_env() {
        fn resolve(reported: Option<&str>, configured: Option<&str>) -> String {
            reported
                .map(ToOwned::to_owned)
                .or_else(|| configured.map(ToOwned::to_owned))
                .unwrap_or_default()
        }

        assert_eq!(
            resolve(Some("claude-opus-5[1m]"), Some("claude-sonnet-5")),
            "claude-opus-5[1m]"
        );
        assert_eq!(resolve(None, Some("gpt-5.6-sol")), "gpt-5.6-sol");
        assert_eq!(resolve(None, None), "");
    }

    #[test]
    fn claude_init_event_reports_model() {
        let init = serde_json::json!({
            "type": "system",
            "subtype": "init",
            "session_id": "sess-1",
            "model": "claude-opus-5[1m]",
        });
        assert_eq!(init["model"].as_str(), Some("claude-opus-5[1m]"));
        // codex 的 thread.started 没有 model 字段，只能靠 auth.model。
        let codex_started = serde_json::json!({"type":"thread.started","thread_id":"t-1"});
        assert!(codex_started["model"].as_str().is_none());
    }

    #[test]
    fn codex_thread_and_final_message_are_parsed() {
        let mut state = CliRunState::default();
        let started = serde_json::json!({"type":"thread.started","thread_id":"thread-1"});
        state.thread_id = started["thread_id"].as_str().map(ToOwned::to_owned);
        let item = serde_json::json!({"type":"item.completed","item":{"type":"agent_message","text":"完成"}});
        state.final_text = item["item"]["text"].as_str().unwrap().to_string();
        assert_eq!(state.thread_id.as_deref(), Some("thread-1"));
        assert_eq!(state.final_text, "完成");
    }

    #[test]
    fn generic_streaming_output_extracts_final_text() {
        let grok = concat!(
            "{\"type\":\"content_block_delta\",\"delta\":{\"text\":\"已\"}}\n",
            "{\"type\":\"content_block_delta\",\"delta\":{\"text\":\"完成\"}}\n"
        );
        assert_eq!(extract_final_text(grok), "已完成");

        let kimi = concat!(
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"处理中\"}]}}\n",
            "{\"type\":\"result\",\"result\":\"处理完成\"}\n"
        );
        assert_eq!(extract_final_text(kimi), "处理完成");
    }

    #[test]
    fn error_output_is_truncated_and_does_not_echo_key_lines() {
        let auth = CliAuth {
            env: vec![("ANTHROPIC_API_KEY", "provider-secret".to_string())],
            ..CliAuth::default()
        };
        let result = redact_secrets(
            &sanitize_error(
                "API_KEY=environment-secret\nnetwork failed: provider-secret",
                None,
            ),
            &auth,
        );
        assert!(!result.contains("secret"));
        assert!(result.contains("network failed"));
    }

    #[test]
    fn codex_db_fallback_only_accepts_official_responses_endpoint() {
        let mut provider = hank_db::ProviderRecord {
            id: "provider-1".to_string(),
            name: "openai-compatible".to_string(),
            provider_type: "openai".to_string(),
            api_key: "secret".to_string(),
            base_url: "https://example.com/v1".to_string(),
            default_model: "model".to_string(),
            models: "{}".to_string(),
            priority: 0,
            enabled: true,
            created_at: chrono::Utc::now(),
        };
        assert!(!codex_provider_is_compatible(&provider));

        provider.base_url = "https://api.openai.com/v1/".to_string();
        assert!(codex_provider_is_compatible(&provider));
        provider.provider_type = "anthropic".to_string();
        assert!(!codex_provider_is_compatible(&provider));
    }

    fn db_config(backend: &str) -> hank_db::AgentCliProfileRecord {
        hank_db::AgentCliProfileRecord {
            id: "profile-1".to_string(),
            backend: backend.to_string(),
            name: "默认".to_string(),
            auth_kind: String::new(),
            api_key: "relay-secret".to_string(),
            base_url: String::new(),
            model: String::new(),
            extra_env: "{}".to_string(),
            is_active: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            updated_by: "admin".to_string(),
        }
    }

    /// 没填 key 的配置必须让位给 agent-cli.env 兜底，否则 admin 存了一份空配置
    /// 并启用，就会把服务器上原本能用的凭据顶掉。
    #[test]
    fn db_config_is_skipped_when_keyless() {
        let mut config = db_config("claude");
        config.api_key = "   ".to_string();
        assert!(auth_from_db_config("claude", &config).is_none());

        config.api_key = "relay-secret".to_string();
        assert!(auth_from_db_config("claude", &config).is_some());
        // 未知后端不允许通过，避免注入到不存在的 CLI。
        assert!(auth_from_db_config("gemini", &config).is_none());
    }

    /// 第三方 Anthropic 中转要 ANTHROPIC_AUTH_TOKEN，官方 key 要 ANTHROPIC_API_KEY，
    /// auth_kind 必须如实落到环境变量名上。
    #[test]
    fn claude_auth_kind_selects_env_var_name() {
        let mut config = db_config("claude");
        config.auth_kind = "ANTHROPIC_AUTH_TOKEN".to_string();
        let auth = auth_from_db_config("claude", &config).expect("配置可用");
        assert!(auth
            .env
            .iter()
            .any(|(key, value)| *key == "ANTHROPIC_AUTH_TOKEN" && value == "relay-secret"));
        assert!(!auth.env.iter().any(|(key, _)| *key == "ANTHROPIC_API_KEY"));

        // auth_kind 为空（历史行）时退回该后端第一个白名单变量。
        config.auth_kind = String::new();
        let auth = auth_from_db_config("claude", &config).expect("配置可用");
        assert!(auth.env.iter().any(|(key, _)| *key == "ANTHROPIC_API_KEY"));

        // 白名单外的变量名不被接受，回退到默认值而不是注入任意环境变量。
        config.auth_kind = "AWS_SECRET_ACCESS_KEY".to_string();
        let auth = auth_from_db_config("claude", &config).expect("配置可用");
        assert!(auth.env.iter().any(|(key, _)| *key == "ANTHROPIC_API_KEY"));
        assert!(!auth
            .env
            .iter()
            .any(|(key, _)| *key == "AWS_SECRET_ACCESS_KEY"));
    }

    /// Codex 的 base_url 进入显式 custom provider；Claude 则通过环境变量读取。
    #[test]
    fn base_url_routing_differs_between_backends() {
        let mut codex = db_config("codex");
        codex.base_url = "https://relay.example.com/v1".to_string();
        let auth = auth_from_db_config("codex", &codex).expect("配置可用");
        assert_eq!(
            auth.base_url.as_deref(),
            Some("https://relay.example.com/v1")
        );
        assert!(!auth.env.iter().any(|(key, _)| *key == "OPENAI_BASE_URL"));

        let mut claude = db_config("claude");
        claude.base_url = "https://relay.example.com".to_string();
        let auth = auth_from_db_config("claude", &claude).expect("配置可用");
        assert!(auth.base_url.is_none());
        assert!(auth.env.iter().any(
            |(key, value)| *key == "ANTHROPIC_BASE_URL" && value == "https://relay.example.com"
        ));
    }

    /// extra_env 由 admin 填写，只能放行白名单键，其他一律丢弃。
    #[test]
    fn extra_env_filters_non_whitelisted_keys() {
        let mut config = db_config("claude");
        config.extra_env = serde_json::json!({
            "ANTHROPIC_DEFAULT_OPUS_MODEL": "claude-opus-5",
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS": "64000",
            "LD_PRELOAD": "/tmp/evil.so",
            "PATH": "/tmp/bin",
        })
        .to_string();
        let auth = auth_from_db_config("claude", &config).expect("配置可用");
        assert!(
            auth.env
                .iter()
                .any(|(key, value)| *key == "ANTHROPIC_DEFAULT_OPUS_MODEL"
                    && value == "claude-opus-5")
        );
        assert!(auth
            .env
            .iter()
            .any(|(key, _)| *key == "CLAUDE_CODE_MAX_OUTPUT_TOKENS"));
        assert!(!auth
            .env
            .iter()
            .any(|(key, _)| matches!(*key, "LD_PRELOAD" | "PATH")));

        // extra_env 是坏 JSON 时不应让整行失效，凭据仍要能用。
        config.extra_env = "not json".to_string();
        let auth = auth_from_db_config("claude", &config).expect("配置可用");
        assert!(auth.env.iter().any(|(key, _)| *key == "ANTHROPIC_API_KEY"));
    }

    /// 库里配置的模型要既进 auth.model（命令行 --model）又进 ANTHROPIC_MODEL，
    /// 与 agent-cli.env 路径的行为保持一致。
    #[test]
    fn db_model_populates_both_flag_and_env_for_claude() {
        let mut config = db_config("claude");
        config.model = "claude-opus-5".to_string();
        let auth = auth_from_db_config("claude", &config).expect("配置可用");
        assert_eq!(auth.model.as_deref(), Some("claude-opus-5"));
        assert!(auth
            .env
            .iter()
            .any(|(key, value)| *key == "ANTHROPIC_MODEL" && value == "claude-opus-5"));

        // codex 只用 --model 参数，不注入 OPENAI_MODEL。
        let mut codex = db_config("codex");
        codex.model = "gpt-5.6".to_string();
        let auth = auth_from_db_config("codex", &codex).expect("配置可用");
        assert_eq!(auth.model.as_deref(), Some("gpt-5.6"));
        assert!(!auth.env.iter().any(|(key, _)| *key == "OPENAI_MODEL"));
    }

    /// 库里的凭据也必须被日志脱敏覆盖，否则 CLI 报错回显会把 key 写进事件流。
    #[test]
    fn db_sourced_secret_is_redacted_from_output() {
        let config = db_config("claude");
        let auth = auth_from_db_config("claude", &config).expect("配置可用");
        let redacted = redact_secrets("error: bad key relay-secret", &auth);
        assert!(!redacted.contains("relay-secret"));
        assert!(redacted.contains("[redacted]"));
    }
}
