//! 本机 Agent CLI runner：只接受固定后端和受控 cwd，不提供任意命令执行入口。

use std::collections::{HashMap, VecDeque};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(unix)]
use std::os::unix::{fs::OpenOptionsExt, fs::PermissionsExt};

use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, Mutex, Notify};

use crate::api::ApiClient;

const SUPPORTED_BACKENDS: [&str; 4] = ["codex", "claude", "grok", "kimi"];
const MAX_PROMPT_BYTES: usize = 256 * 1024;
/// 单行上限：stream-json 里一条 tool_result 可能带整个文件内容。超限只丢这一行，
/// 不杀进程——探索型任务读几十个文件是常态，不该按异常处理。
const DEFAULT_MAX_LINE_BYTES: usize = 1024 * 1024;
/// 整流上限（真正的 runaway 保护）。远高于正常任务量级，撞到才终止进程组。
const DEFAULT_MAX_STREAM_BYTES: usize = 64 * 1024 * 1024;
/// 回传给 server 的 stdout 只保留尾部：final_text 兜底解析找的 result 事件在流末尾，
/// 保留头部等于必然拿不到（旧实现的方向错误）。
const RETAIN_STDOUT_BYTES: usize = 256 * 1024;
const RETAIN_STDERR_BYTES: usize = 64 * 1024;
const READ_CHUNK_BYTES: usize = 64 * 1024;
const STREAM_QUEUE_CAPACITY: usize = 256;
const MAX_PENDING_CANCELLATIONS: usize = 1024;

/// 输出闸门参数。默认值适用于全仓库通读类任务，可由配置文件覆盖。
#[derive(Clone, Copy)]
pub struct AgentLimits {
    pub max_line_bytes: usize,
    pub max_stream_bytes: usize,
}

impl Default for AgentLimits {
    fn default() -> Self {
        Self {
            max_line_bytes: DEFAULT_MAX_LINE_BYTES,
            max_stream_bytes: DEFAULT_MAX_STREAM_BYTES,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct AgentRunInput {
    pub backend: String,
    pub prompt: String,
    pub cwd: Option<String>,
    pub thread_id: Option<String>,
    pub model: Option<String>,
}

pub struct AgentOutcome {
    pub content: String,
    pub is_error: bool,
}

#[derive(Default)]
struct JobState {
    running: HashMap<String, Arc<JobControl>>,
    cancelled_before_start: VecDeque<String>,
}

#[derive(Default)]
struct JobControl {
    cancelled: AtomicBool,
    notify: Notify,
}

impl JobControl {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    async fn cancelled(&self) {
        if self.cancelled.load(Ordering::Acquire) {
            return;
        }
        self.notify.notified().await;
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

pub struct AgentRunner {
    work_root: Option<PathBuf>,
    data_dir: PathBuf,
    backends: Vec<String>,
    limits: AgentLimits,
    jobs: Mutex<JobState>,
}

impl AgentRunner {
    pub fn with_limits(
        work_dir: Option<String>,
        data_dir: PathBuf,
        backends: Vec<String>,
        limits: AgentLimits,
    ) -> Self {
        let work_root = work_dir
            .map(PathBuf::from)
            .and_then(|path| std::fs::canonicalize(path).ok())
            .filter(|path| path.is_dir());
        Self {
            work_root,
            data_dir,
            backends,
            limits,
            jobs: Mutex::new(JobState::default()),
        }
    }

    pub async fn cancel(&self, request_id: &str) -> bool {
        let mut jobs = self.jobs.lock().await;
        if let Some(control) = jobs.running.get(request_id) {
            control.cancel();
            true
        } else {
            // agent_run 与 agent_cancel 可能在同一批 poll 中并发调度，记住抢跑的取消。
            if !jobs
                .cancelled_before_start
                .iter()
                .any(|value| value == request_id)
            {
                if jobs.cancelled_before_start.len() >= MAX_PENDING_CANCELLATIONS {
                    jobs.cancelled_before_start.pop_front();
                }
                jobs.cancelled_before_start
                    .push_back(request_id.to_string());
            }
            true
        }
    }

    pub async fn run(
        &self,
        api: Arc<ApiClient>,
        request_id: &str,
        input: AgentRunInput,
    ) -> AgentOutcome {
        match self.run_inner(api, request_id, input).await {
            Ok(outcome) => outcome,
            Err(message) => AgentOutcome {
                content: message,
                is_error: true,
            },
        }
    }

    async fn run_inner(
        &self,
        api: Arc<ApiClient>,
        request_id: &str,
        input: AgentRunInput,
    ) -> Result<AgentOutcome, String> {
        let backend = input.backend.trim().to_ascii_lowercase();
        if !SUPPORTED_BACKENDS.contains(&backend.as_str()) {
            return Err(format!(
                "不支持的 Agent 后端: {backend}（允许: codex/claude/grok/kimi）"
            ));
        }
        if !self.backends.iter().any(|value| value == &backend) {
            return Err(format!("本节点未启用 Agent 后端: {backend}"));
        }
        if input.prompt.len() > MAX_PROMPT_BYTES {
            return Err(format!("Agent prompt 超过 {} 字节上限", MAX_PROMPT_BYTES));
        }
        let cwd = self.resolve_cwd(input.cwd.as_deref())?;
        let mut spec = build_command_spec(
            &backend,
            &cwd,
            request_id,
            &input.prompt,
            input.thread_id.as_deref(),
            input.model.as_deref(),
            &self.data_dir,
        )?;
        let control = self.start_job(request_id).await;

        let result = self
            .execute_process(
                api,
                request_id,
                &backend,
                &cwd,
                &input.prompt,
                &control,
                &mut spec,
            )
            .await;
        self.finish_job(request_id).await;
        if let Some(path) = spec.cleanup_path {
            let _ = std::fs::remove_file(path);
        }
        result
    }

    fn resolve_cwd(&self, requested: Option<&str>) -> Result<PathBuf, String> {
        let root = self
            .work_root
            .as_ref()
            .ok_or("hank-cli 没有有效 work_dir，不能运行本机 Agent")?;
        let requested = requested
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| root.clone());
        let cwd = std::fs::canonicalize(&requested)
            .map_err(|error| format!("Agent cwd 不存在 {}: {error}", requested.display()))?;
        if !cwd.starts_with(root) {
            return Err(format!(
                "Agent cwd {} 超出节点工作目录 {}",
                cwd.display(),
                root.display()
            ));
        }
        Ok(cwd)
    }

    async fn start_job(&self, request_id: &str) -> Arc<JobControl> {
        let mut jobs = self.jobs.lock().await;
        let control = Arc::new(JobControl::default());
        if let Some(index) = jobs
            .cancelled_before_start
            .iter()
            .position(|value| value == request_id)
        {
            jobs.cancelled_before_start.remove(index);
            control.cancel();
        }
        jobs.running.insert(request_id.to_string(), control.clone());
        control
    }

    async fn finish_job(&self, request_id: &str) {
        self.jobs.lock().await.running.remove(request_id);
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_process(
        &self,
        api: Arc<ApiClient>,
        request_id: &str,
        backend: &str,
        cwd: &Path,
        prompt: &str,
        control: &Arc<JobControl>,
        spec: &mut CommandSpec,
    ) -> Result<AgentOutcome, String> {
        if control.is_cancelled() {
            return Ok(cancelled_outcome(backend));
        }
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .current_dir(cwd)
            .stdin(if spec.write_prompt_to_stdin {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.as_std_mut().process_group(0);

        let mut child = command
            .spawn()
            .map_err(|error| format!("启动本机 {backend} 失败: {error}"))?;
        let pid = child.id();
        if spec.write_prompt_to_stdin {
            let mut stdin = child.stdin.take().ok_or("本机 Agent stdin 不可用")?;
            stdin
                .write_all(prompt.as_bytes())
                .await
                .map_err(|error| format!("发送本机 Agent prompt 失败: {error}"))?;
            drop(stdin);
        }

        post_event(
            &api,
            request_id,
            serde_json::json!({
                "stream": "control",
                "line": serde_json::json!({
                    "type": "trace.agent.started",
                    "backend": backend,
                    "session_id": spec.session_id,
                }).to_string(),
            }),
        )
        .await;

        let stdout = child.stdout.take().ok_or("本机 Agent stdout 不可用")?;
        let stderr = child.stderr.take().ok_or("本机 Agent stderr 不可用")?;
        let (line_tx, mut line_rx) = mpsc::channel(STREAM_QUEUE_CAPACITY);
        let stdout_task = tokio::spawn(read_lines(stdout, "stdout", self.limits, line_tx.clone()));
        let stderr_task = tokio::spawn(read_lines(stderr, "stderr", self.limits, line_tx.clone()));
        drop(line_tx);

        let mut stdout_tail = TailBuffer::new(RETAIN_STDOUT_BYTES);
        let mut stderr_tail = TailBuffer::new(RETAIN_STDERR_BYTES);
        let mut dropped_lines = 0usize;
        let mut cancelled = false;
        let mut output_limited = false;
        let status: ExitStatus;

        loop {
            if let Some(done) = child
                .try_wait()
                .map_err(|error| format!("查询本机 Agent 状态失败: {error}"))?
            {
                status = done;
                break;
            }
            tokio::select! {
                _ = control.cancelled() => {
                    cancelled = true;
                    terminate_process_group(&mut child, pid).await;
                    status = child.wait().await
                        .map_err(|error| format!("等待已取消的本机 Agent 失败: {error}"))?;
                    break;
                }
                chunk = line_rx.recv() => {
                    match chunk {
                        Some(StreamChunk::Line(stream, line)) => {
                            if stream == "stdout" {
                                stdout_tail.push(&line);
                            } else {
                                stderr_tail.push(&line);
                            }
                            post_event(
                                &api,
                                request_id,
                                serde_json::json!({ "stream": stream, "line": line }),
                            ).await;
                        }
                        Some(StreamChunk::Dropped) => {
                            dropped_lines += 1;
                        }
                        Some(StreamChunk::Limit(stream)) => {
                            output_limited = true;
                            stderr_tail.push(&format!("{stream} 超过整流安全上限"));
                            terminate_process_group(&mut child, pid).await;
                            status = child.wait().await
                                .map_err(|error| format!("等待输出超限的本机 Agent 失败: {error}"))?;
                            break;
                        }
                        None => {}
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }

        while let Some(chunk) = line_rx.recv().await {
            match chunk {
                StreamChunk::Line(stream, line) => {
                    if stream == "stdout" {
                        stdout_tail.push(&line);
                    } else {
                        stderr_tail.push(&line);
                    }
                    post_event(
                        &api,
                        request_id,
                        serde_json::json!({ "stream": stream, "line": line }),
                    )
                    .await;
                }
                StreamChunk::Dropped => {
                    dropped_lines += 1;
                }
                StreamChunk::Limit(stream) => {
                    output_limited = true;
                    stderr_tail.push(&format!("{stream} 超过整流安全上限"));
                }
            }
        }
        let _ = stdout_task.await;
        let _ = stderr_task.await;

        if dropped_lines > 0 {
            tracing::warn!(
                request_id,
                backend,
                "丢弃 {dropped_lines} 行超长输出（单行上限 {} 字节）",
                self.limits.max_line_bytes
            );
        }

        let content = serde_json::json!({
            "backend": backend,
            "exit_code": status.code(),
            "cancelled": cancelled,
            "output_limited": output_limited,
            "dropped_lines": dropped_lines,
            "stdout": stdout_tail.into_string(),
            "stderr": stderr_tail.into_string(),
        })
        .to_string();
        Ok(AgentOutcome {
            content,
            is_error: cancelled || output_limited || !status.success(),
        })
    }
}

fn cancelled_outcome(backend: &str) -> AgentOutcome {
    AgentOutcome {
        content: serde_json::json!({
            "backend": backend,
            "exit_code": serde_json::Value::Null,
            "cancelled": true,
            "output_limited": false,
            "stdout": "",
            "stderr": "",
        })
        .to_string(),
        is_error: true,
    }
}

struct CommandSpec {
    program: String,
    args: Vec<String>,
    write_prompt_to_stdin: bool,
    cleanup_path: Option<PathBuf>,
    session_id: Option<String>,
}

#[allow(clippy::too_many_arguments)]
fn build_command_spec(
    backend: &str,
    cwd: &Path,
    request_id: &str,
    prompt: &str,
    thread_id: Option<&str>,
    model: Option<&str>,
    data_dir: &Path,
) -> Result<CommandSpec, String> {
    match backend {
        "codex" => {
            let mut args = vec![
                "-a".into(),
                "never".into(),
                "-s".into(),
                "workspace-write".into(),
                "exec".into(),
            ];
            if let Some(thread_id) = thread_id {
                args.extend([
                    "resume".into(),
                    "--json".into(),
                    "--skip-git-repo-check".into(),
                ]);
                if let Some(model) = model {
                    args.extend(["--model".into(), model.into()]);
                }
                args.extend([thread_id.into(), "-".into()]);
            } else {
                args.extend([
                    "--json".into(),
                    "--color".into(),
                    "never".into(),
                    "--skip-git-repo-check".into(),
                    "-C".into(),
                    cwd.display().to_string(),
                ]);
                if let Some(model) = model {
                    args.extend(["--model".into(), model.into()]);
                }
                args.push("-".into());
            }
            Ok(CommandSpec {
                program: "codex".into(),
                args,
                write_prompt_to_stdin: true,
                cleanup_path: None,
                session_id: thread_id.map(ToOwned::to_owned),
            })
        }
        "claude" => {
            let mut args = vec![
                "-p".into(),
                "--output-format".into(),
                "stream-json".into(),
                "--verbose".into(),
                "--permission-mode".into(),
                "dontAsk".into(),
                "--strict-mcp-config".into(),
                "--mcp-config".into(),
                r#"{"mcpServers":{}}"#.into(),
            ];
            let session_id = thread_id.unwrap_or(request_id).to_string();
            if let Some(thread_id) = thread_id {
                args.extend(["--resume".into(), thread_id.into()]);
            } else {
                args.extend(["--session-id".into(), session_id.clone()]);
            }
            if let Some(model) = model {
                args.extend(["--model".into(), model.into()]);
            }
            Ok(CommandSpec {
                program: "claude".into(),
                args,
                write_prompt_to_stdin: true,
                cleanup_path: None,
                session_id: Some(session_id),
            })
        }
        "grok" => {
            let prompt_dir = data_dir.join("agent-prompts");
            std::fs::create_dir_all(&prompt_dir)
                .map_err(|error| format!("创建 Grok prompt 目录失败: {error}"))?;
            #[cfg(unix)]
            std::fs::set_permissions(&prompt_dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|error| format!("设置 Grok prompt 目录权限失败: {error}"))?;
            let prompt_path = prompt_dir.join(format!("{request_id}.txt"));
            write_private_file(&prompt_path, prompt)?;
            let session_id = thread_id.unwrap_or(request_id).to_string();
            let mut args = vec![
                "--cwd".into(),
                cwd.display().to_string(),
                "--output-format".into(),
                "streaming-messages-json".into(),
                "--include-partial-messages".into(),
                "--permission-mode".into(),
                "dontAsk".into(),
                "--prompt-file".into(),
                prompt_path.display().to_string(),
            ];
            if let Some(thread_id) = thread_id {
                args.extend(["--resume".into(), thread_id.into()]);
            } else {
                args.extend(["--session-id".into(), session_id.clone()]);
            }
            if let Some(model) = model {
                args.extend(["--model".into(), model.into()]);
            }
            Ok(CommandSpec {
                program: "grok".into(),
                args,
                write_prompt_to_stdin: false,
                cleanup_path: Some(prompt_path),
                session_id: Some(session_id),
            })
        }
        "kimi" => {
            let mut args = vec!["--output-format".into(), "stream-json".into()];
            if let Some(thread_id) = thread_id {
                args.extend(["--session".into(), thread_id.into()]);
            }
            if let Some(model) = model {
                args.extend(["--model".into(), model.into()]);
            }
            // Kimi 0.31 的 prompt 模式没有 stdin/prompt-file 选项；Command 直传参数，不经过 shell。
            args.extend(["--prompt".into(), prompt.into()]);
            Ok(CommandSpec {
                program: "kimi".into(),
                args,
                write_prompt_to_stdin: false,
                cleanup_path: None,
                session_id: thread_id.map(ToOwned::to_owned),
            })
        }
        _ => Err(format!("不支持的 Agent 后端: {backend}")),
    }
}

fn write_private_file(path: &Path, content: &str) -> Result<(), String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .map_err(|error| format!("创建私有 prompt 文件失败: {error}"))?;
    file.write_all(content.as_bytes())
        .map_err(|error| format!("写入私有 prompt 文件失败: {error}"))
}

enum StreamChunk {
    Line(&'static str, String),
    /// 单行超限被丢弃：只影响这一行，任务继续。
    Dropped,
    /// 整流超限：runaway 保护，主循环会终止进程组。
    Limit(&'static str),
}

/// 只保留尾部若干字节的行缓冲。按行淘汰，不做半行切断。
struct TailBuffer {
    lines: VecDeque<String>,
    bytes: usize,
    limit: usize,
}

impl TailBuffer {
    fn new(limit: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            bytes: 0,
            limit,
        }
    }

    fn push(&mut self, line: &str) {
        if self.limit == 0 {
            return;
        }
        let mut line = line.to_string();
        if line.len() + 1 > self.limit {
            // 单行本身就超过保留窗口：留尾部，按字符边界切。
            let mut cut = line.len() - (self.limit - 1);
            while cut < line.len() && !line.is_char_boundary(cut) {
                cut += 1;
            }
            line = line.split_off(cut);
        }
        self.bytes = self.bytes.saturating_add(line.len() + 1);
        self.lines.push_back(line);
        while self.bytes > self.limit {
            match self.lines.pop_front() {
                Some(dropped) => self.bytes = self.bytes.saturating_sub(dropped.len() + 1),
                None => break,
            }
        }
    }

    fn into_string(self) -> String {
        let mut out = String::with_capacity(self.bytes);
        for line in self.lines {
            out.push_str(&line);
            out.push('\n');
        }
        out
    }
}

/// 逐行读取子进程输出：单行超 `max_line_bytes` 丢弃该行并继续；整流累计超
/// `max_stream_bytes` 才上报 Limit（由主循环终止进程组）。
async fn read_lines<R>(
    reader: R,
    stream: &'static str,
    limits: AgentLimits,
    tx: mpsc::Sender<StreamChunk>,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut reader = BufReader::with_capacity(READ_CHUNK_BYTES, reader);
    let mut pending: Vec<u8> = Vec::new();
    let mut pending_overflow = false;
    let mut total = 0usize;
    let mut buf = vec![0u8; READ_CHUNK_BYTES];

    loop {
        let read = match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) => break,
        };
        total = total.saturating_add(read);
        if total > limits.max_stream_bytes {
            let _ = tx.send(StreamChunk::Limit(stream)).await;
            return;
        }
        let mut rest = &buf[..read];
        while let Some(index) = rest.iter().position(|byte| *byte == b'\n') {
            let (head, tail) = rest.split_at(index);
            rest = &tail[1..];
            let overflow = pending_overflow || pending.len() + head.len() > limits.max_line_bytes;
            if overflow {
                pending.clear();
                pending_overflow = false;
                if tx.send(StreamChunk::Dropped).await.is_err() {
                    return;
                }
                continue;
            }
            pending.extend_from_slice(head);
            let line = String::from_utf8_lossy(&pending).into_owned();
            pending.clear();
            if tx.send(StreamChunk::Line(stream, line)).await.is_err() {
                return;
            }
        }
        if pending_overflow {
            continue;
        }
        if pending.len() + rest.len() > limits.max_line_bytes {
            // 半行已超限：丢掉累积部分，等这一行结束再恢复正常。
            pending.clear();
            pending_overflow = true;
            continue;
        }
        pending.extend_from_slice(rest);
    }

    if pending_overflow {
        let _ = tx.send(StreamChunk::Dropped).await;
    } else if !pending.is_empty() {
        let line = String::from_utf8_lossy(&pending).into_owned();
        let _ = tx.send(StreamChunk::Line(stream, line)).await;
    }
}

async fn post_event(api: &ApiClient, request_id: &str, event: serde_json::Value) {
    if let Err(error) = api.post_agent_event(request_id, &event).await {
        tracing::warn!(request_id, "上报本机 Agent 事件失败: {error}");
    }
}

async fn terminate_process_group(child: &mut Child, pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
        for _ in 0..10 {
            if child.try_wait().ok().flatten().is_some() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
        return;
    }
    let _ = child.kill().await;
}

pub fn detect_backends(configured: Option<&[String]>) -> Vec<String> {
    let requested: Vec<String> = configured
        .map(|values| {
            values
                .iter()
                .map(|value| value.trim().to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_else(|| {
            SUPPORTED_BACKENDS
                .iter()
                .map(|value| value.to_string())
                .collect()
        });
    SUPPORTED_BACKENDS
        .iter()
        .filter(|backend| requested.iter().any(|value| value == **backend))
        .filter(|backend| executable_on_path(backend))
        .map(|backend| backend.to_string())
        .collect()
}

fn executable_on_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(program);
        candidate.is_file() && {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                candidate
                    .metadata()
                    .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
            }
            #[cfg(not(unix))]
            {
                true
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn collect(input: &str, limits: AgentLimits) -> (Vec<String>, usize, bool) {
        let (tx, mut rx) = mpsc::channel(1024);
        let reader = std::io::Cursor::new(input.as_bytes().to_vec());
        let task = tokio::spawn(read_lines(reader, "stdout", limits, tx));
        let mut lines = Vec::new();
        let mut dropped = 0usize;
        let mut limited = false;
        while let Some(chunk) = rx.recv().await {
            match chunk {
                StreamChunk::Line(_, line) => lines.push(line),
                StreamChunk::Dropped => dropped += 1,
                StreamChunk::Limit(_) => limited = true,
            }
        }
        let _ = task.await;
        (lines, dropped, limited)
    }

    #[tokio::test]
    async fn oversized_line_is_dropped_without_stopping_stream() {
        let limits = AgentLimits {
            max_line_bytes: 64,
            max_stream_bytes: 1024 * 1024,
        };
        let input = format!("first\n{}\nlast\n", "x".repeat(200));
        let (lines, dropped, limited) = collect(&input, limits).await;
        assert_eq!(lines, vec!["first".to_string(), "last".to_string()]);
        assert_eq!(dropped, 1);
        // 单行超限不是 runaway：进程必须继续跑完。
        assert!(!limited);
    }

    #[tokio::test]
    async fn stream_limit_reports_runaway() {
        let limits = AgentLimits {
            max_line_bytes: 1024,
            max_stream_bytes: 128,
        };
        let input = "abcdefgh\n".repeat(64);
        let (_, _, limited) = collect(&input, limits).await;
        assert!(limited);
    }

    #[tokio::test]
    async fn reader_handles_missing_trailing_newline() {
        let (lines, dropped, limited) = collect("a\nb", AgentLimits::default()).await;
        assert_eq!(lines, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(dropped, 0);
        assert!(!limited);
    }

    #[test]
    fn tail_buffer_keeps_the_end_not_the_head() {
        // final_text 兜底解析要的 result 事件在流尾部，保留头部等于拿不到。
        let mut tail = TailBuffer::new(16);
        for line in ["aaaa", "bbbb", "cccc", "result"] {
            tail.push(line);
        }
        let text = tail.into_string();
        assert!(text.contains("result"));
        assert!(!text.contains("aaaa"));
        assert!(text.len() <= 16);
    }

    #[test]
    fn tail_buffer_truncates_single_oversized_line_on_char_boundary() {
        let mut tail = TailBuffer::new(8);
        tail.push("中文中文中文");
        let text = tail.into_string();
        // 切点必须落在字符边界上，否则 from_utf8 会炸。
        assert!(text.chars().all(|c| c == '中' || c == '文' || c == '\n'));
        assert!(text.len() <= 8);
    }

    #[test]
    fn config_limits_keep_stream_above_line() {
        let limits = crate::config::agent_limits_for_test(Some(8), Some(2));
        assert_eq!(limits.max_line_bytes, 8 * 1024 * 1024);
        assert_eq!(limits.max_stream_bytes, 8 * 1024 * 1024);
    }

    #[test]
    fn codex_spec_uses_workspace_sandbox_without_bypass() {
        let spec = build_command_spec(
            "codex",
            Path::new("/tmp/project"),
            "00000000-0000-0000-0000-000000000001",
            "test",
            None,
            None,
            Path::new("/tmp"),
        )
        .unwrap();
        assert!(spec
            .args
            .windows(2)
            .any(|pair| pair == ["-s", "workspace-write"]));
        assert!(!spec
            .args
            .iter()
            .any(|arg| arg.contains("dangerously-bypass")));
        assert!(spec.write_prompt_to_stdin);
    }

    #[test]
    fn configured_backends_are_an_allowlist() {
        let configured = vec!["codex".to_string(), "shell".to_string()];
        let detected = detect_backends(Some(&configured));
        assert!(!detected.iter().any(|value| value == "shell"));
    }

    #[test]
    fn kimi_prompt_mode_does_not_pass_conflicting_permission_flag() {
        let spec = build_command_spec(
            "kimi",
            Path::new("/tmp/project"),
            "00000000-0000-0000-0000-000000000001",
            "test",
            None,
            None,
            Path::new("/tmp"),
        )
        .unwrap();
        assert!(spec
            .args
            .windows(2)
            .any(|pair| pair == ["--prompt", "test"]));
        assert!(!spec.args.iter().any(|arg| arg == "--auto"));
        assert!(!spec.args.iter().any(|arg| arg == "--yolo"));
    }

    #[test]
    fn resolve_cwd_rejects_paths_outside_work_root() {
        let root = std::env::temp_dir().join(format!("hank-cli-agent-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let runner = AgentRunner::with_limits(
            Some(root.display().to_string()),
            root.join("data"),
            vec!["codex".into()],
            AgentLimits::default(),
        );
        let outside =
            std::env::temp_dir().join(format!("hank-cli-agent-outside-{}", std::process::id()));
        std::fs::create_dir_all(&outside).unwrap();
        let err = runner
            .resolve_cwd(Some(outside.to_str().unwrap()))
            .unwrap_err();
        assert!(err.contains("超出节点工作目录"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn resolve_cwd_defaults_to_work_root_when_null() {
        let root =
            std::env::temp_dir().join(format!("hank-cli-agent-default-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let runner = AgentRunner::with_limits(
            Some(root.display().to_string()),
            root.join("data"),
            vec!["claude".into()],
            AgentLimits::default(),
        );
        let cwd = runner.resolve_cwd(None).unwrap();
        assert_eq!(cwd, std::fs::canonicalize(&root).unwrap());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unsupported_backend_is_rejected_by_command_spec() {
        let result = build_command_spec(
            "bash",
            Path::new("/tmp/project"),
            "req",
            "hi",
            None,
            None,
            Path::new("/tmp"),
        );
        match result {
            Ok(_) => panic!("bash backend should be rejected"),
            Err(err) => assert!(err.contains("不支持"), "{err}"),
        }
    }
}
