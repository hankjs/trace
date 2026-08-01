//! 飞书外部 Agent 后端：在逐话题 bubblewrap 沙箱中运行 Codex / Claude Code。

use crate::chat::{ChatTurnHandle, EventBuffer};
use crate::AppState;
use anyhow::{anyhow, bail, Context, Result};
use code_agent::{AgentEvent, FileChange, FileChangeKind, RunStatus};
use hank_db::Session;
use hank_provider::ContentBlock;
use serde::Serialize;
use std::collections::HashSet;
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
fn auth_from_db_config(
    backend: &str,
    config: &hank_db::AgentCliProfileRecord,
) -> Option<CliAuth> {
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
            // Codex 0.146 不读 OPENAI_BASE_URL，要转成 -c openai_base_url 配置。
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
    let mut command = build_command(
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
            tracing::warn!(session_id, backend, "回写外部 Agent provider/model 失败: {error:#}");
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
            emit_failed(state, session_id, &run_id, "外部 Agent 输出超过安全上限").await;
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

#[derive(Clone, Copy)]
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

fn build_command(
    state: &Arc<AppState>,
    backend: &str,
    work_dir: &Path,
    state_dir: &Path,
    git_link: Option<&GitLink>,
    auth: &CliAuth,
    previous_thread: Option<&str>,
) -> Result<Command> {
    let cfg = &state.config.server_agent;
    let executable = match backend {
        "codex" => Path::new(&cfg.agent_cli_root).join("codex/current/bin/codex"),
        "claude" => Path::new(&cfg.agent_cli_root).join("claude/current/bin/claude"),
        _ => bail!("不支持的外部 Agent 后端: {backend}"),
    };
    let mut command = Command::new("sudo");
    command.env_clear();
    let mut preserved = vec![
        "HOME",
        "CODEX_HOME",
        "CLAUDE_CONFIG_DIR",
        "CARGO_HOME",
        "GIT_OPTIONAL_LOCKS",
        "PATH",
        "RUSTUP_HOME",
        "UV_CACHE_DIR",
    ];
    command.env("HOME", "/agent-home");
    command.env("CODEX_HOME", "/agent-home/.codex");
    command.env("CLAUDE_CONFIG_DIR", "/agent-home/.claude");
    command.env("CARGO_HOME", "/agent-home/.cargo-cache");
    command.env("GIT_OPTIONAL_LOCKS", "0");
    command.env(
        "RUSTUP_HOME",
        format!("/home/{}/.rustup", cfg.execution_user),
    );
    command.env("UV_CACHE_DIR", "/agent-home/.uv-cache");
    command.env(
        "PATH",
        format!(
            "{}/codex/current/bin:{}/claude/current/bin:/home/{}/.cargo/bin:/home/{}/.local/bin:/usr/local/bin:/usr/bin:/bin",
            cfg.agent_cli_root, cfg.agent_cli_root, cfg.execution_user, cfg.execution_user
        ),
    );
    for (key, value) in &auth.env {
        command.env(key, value);
        if !preserved.contains(key) {
            preserved.push(key);
        }
    }
    let preserve_arg = format!("--preserve-env={}", preserved.join(","));
    command.args([
        "-n",
        &preserve_arg,
        "-u",
        &cfg.execution_user,
        &cfg.agent_sandbox_bin,
    ]);
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
                let value = serde_json::to_string(base_url).context("编码 Codex base URL")?;
                command.args(["-c", &format!("openai_base_url={value}")]);
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
    Ok(command)
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
    if backend == "codex" {
        handle_codex_event(state, session_id, auth, &value, run).await;
    } else {
        handle_claude_event(state, session_id, auth, &value, run).await;
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
                    // Codex 0.146 不直接读取该环境变量，要转成 openai_base_url 配置。
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

/// 新话题未明确指定 CLI 时，优先选择当前确实有可用凭据的后端。
pub(crate) async fn preferred_backend(state: &AppState) -> &'static str {
    // admin 里启用的配置与环境变量同权：两处任一可用都算这个后端能跑，
    // 否则 admin 配好 claude 后新话题仍会被路由到没凭据的 codex。
    let profiles = state.db.list_agent_cli_profiles().await.unwrap_or_default();
    let db_ready = |backend: &str| {
        profiles.iter().any(|profile| {
            profile.backend == backend
                && profile.is_active
                && !profile.api_key.trim().is_empty()
        })
    };
    let codex_env = std::env::var("OPENAI_API_KEY").is_ok_and(|value| !value.trim().is_empty());
    let claude_env = [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
    ]
    .iter()
    .any(|key| std::env::var(key).is_ok_and(|value| !value.trim().is_empty()));
    let providers = state.db.list_providers_ordered().await.unwrap_or_default();
    let codex_provider = providers.iter().any(codex_provider_is_compatible);
    let claude_provider = providers
        .iter()
        .any(|provider| provider.enabled && provider.provider_type == "anthropic");

    if db_ready("codex") || codex_env || codex_provider {
        "codex"
    } else if db_ready("claude") || claude_env || claude_provider {
        "claude"
    } else {
        // 保留一个确定的失败路径，由 resolve_cli_auth 返回可操作的配置说明。
        "codex"
    }
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

    /// Codex 0.146 不读 OPENAI_BASE_URL，base_url 必须走 -c openai_base_url；
    /// Claude 则相反，用环境变量。
    #[test]
    fn base_url_routing_differs_between_backends() {
        let mut codex = db_config("codex");
        codex.base_url = "https://relay.example.com/v1".to_string();
        let auth = auth_from_db_config("codex", &codex).expect("配置可用");
        assert_eq!(auth.base_url.as_deref(), Some("https://relay.example.com/v1"));
        assert!(!auth.env.iter().any(|(key, _)| *key == "OPENAI_BASE_URL"));

        let mut claude = db_config("claude");
        claude.base_url = "https://relay.example.com".to_string();
        let auth = auth_from_db_config("claude", &claude).expect("配置可用");
        assert!(auth.base_url.is_none());
        assert!(auth
            .env
            .iter()
            .any(|(key, value)| *key == "ANTHROPIC_BASE_URL"
                && value == "https://relay.example.com"));
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
        assert!(auth
            .env
            .iter()
            .any(|(key, value)| *key == "ANTHROPIC_DEFAULT_OPUS_MODEL" && value == "claude-opus-5"));
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
