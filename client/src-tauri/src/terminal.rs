//! 内置终端：基于 portable-pty 的 PTY 会话管理。
//! 每个会话持有 shell 子进程、写入端、滚动缓冲（256KB 环形），
//! 输出通过 `term-output/{id}` 事件实时推给前端。

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

const SCROLLBACK_CAP: usize = 256 * 1024;

pub struct TermSession {
    pub id: String,
    pub writer: Box<dyn Write + Send>,
    pub master: Box<dyn MasterPty + Send>,
    pub child_pid: u32,
    pub shell: String,
    pub cwd: String,
    pub created_at: String,
    pub scrollback: Arc<Mutex<VecDeque<u8>>>,
    pub alive: Arc<AtomicBool>,
    /// PTY 当前尺寸，随 term_create/term_resize 更新
    pub cols: u16,
    pub rows: u16,
}

#[derive(Default)]
pub struct TermManager {
    pub sessions: Mutex<HashMap<String, TermSession>>,
}

#[derive(Serialize, Clone)]
pub struct TermInfo {
    pub id: String,
    pub shell: String,
    pub cwd: String,
    pub foreground_cmd: String,
    pub alive: bool,
    pub created_at: String,
    pub cols: u16,
    pub rows: u16,
}

/// macOS 下沿 `ps` 树找 child_pid 后代链最深的进程，返回 (pid, comm)。
#[cfg(target_os = "macos")]
fn deepest_descendant(child_pid: u32) -> Option<(u32, String)> {
    let output = std::process::Command::new("ps")
        .args(["-Ao", "pid=,ppid=,comm="])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let text = String::from_utf8_lossy(&output.stdout);
    // ppid -> Vec<(pid, comm)>
    let mut children: HashMap<u32, Vec<(u32, String)>> = HashMap::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(pid), Some(ppid)) = (
            parts.next().and_then(|s| s.parse::<u32>().ok()),
            parts.next().and_then(|s| s.parse::<u32>().ok()),
        ) else {
            continue;
        };
        let comm = parts.next().unwrap_or("").to_string();
        children.entry(ppid).or_default().push((pid, comm));
    }
    // 沿后代链走到最深：每层取第一个子进程
    let mut cur = child_pid;
    let mut deepest = (child_pid, String::new());
    let mut guard = 0;
    while let Some(kids) = children.get(&cur) {
        if kids.is_empty() || guard > 64 {
            break;
        }
        guard += 1;
        deepest = kids[0].clone();
        cur = deepest.0;
    }
    Some(deepest)
}

/// macOS 下用 `ps -Ao pid=,ppid=,comm=` 找到 child_pid 后代链最深进程的 comm。
#[cfg(target_os = "macos")]
fn foreground_cmd(child_pid: u32, shell: &str) -> String {
    let fallback = || {
        std::path::Path::new(shell)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| shell.to_string())
    };
    match deepest_descendant(child_pid) {
        Some((_, comm)) if !comm.is_empty() => std::path::Path::new(&comm)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or(comm),
        _ => fallback(),
    }
}

/// macOS 下取前台进程（后代链最深）的当前工作目录：`lsof -a -p <pid> -d cwd -Fn`。
#[cfg(target_os = "macos")]
fn foreground_cwd(s: &TermSession) -> String {
    if let Some((pid, _)) = deepest_descendant(s.child_pid) {
        if let Ok(o) = std::process::Command::new("lsof")
            .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
            .output()
        {
            if o.status.success() {
                let text = String::from_utf8_lossy(&o.stdout);
                // 输出形如 "p<pid>\nn</path>"，取 n 行
                for line in text.lines() {
                    if let Some(path) = line.strip_prefix('n') {
                        if !path.is_empty() {
                            return path.to_string();
                        }
                    }
                }
            }
        }
    }
    s.cwd.clone()
}

#[cfg(not(target_os = "macos"))]
fn foreground_cmd(_child_pid: u32, shell: &str) -> String {
    std::path::Path::new(shell)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| shell.to_string())
}

fn session_info(s: &TermSession) -> TermInfo {
    TermInfo {
        id: s.id.clone(),
        shell: s.shell.clone(),
        // 展示用实时 cwd（前台进程 cd 过也能跟上），代价是每次一次 lsof，量级可忽略
        cwd: foreground_cwd(s),
        foreground_cmd: foreground_cmd(s.child_pid, &s.shell),
        alive: s.alive.load(Ordering::SeqCst),
        created_at: s.created_at.clone(),
        cols: s.cols,
        rows: s.rows,
    }
}

fn append_scrollback(buf: &Arc<Mutex<VecDeque<u8>>>, data: &[u8]) {
    let mut b = buf.lock().unwrap();
    b.extend(data);
    while b.len() > SCROLLBACK_CAP {
        b.pop_front();
    }
}

// ─── 终端通知捕获（OSC 9 / 777 / 133 与 BEL）───────────────────────────────
//
// 在 reader 线程的 PTY 字节流上增量扫描，与视图是否附着无关——无头终端（如微信
// 托管的 kimi CLI）也能捕获通知。结果经全局 `term-notify` 事件发给前端统一上报。

/// 命令耗时超过该值才通知（失败命令不受限）
const CMD_NOTIFY_MIN_MS: u128 = 30_000;
/// BEL 响铃去重窗口
const BELL_DEDUPE_MS: u128 = 10_000;

/// 全局 `term-notify` 事件负载
#[derive(Serialize, Clone)]
struct TermNotifyEvent {
    id: String,
    kind: String,
    title: String,
    body: String,
}

/// PTY 字节流上的增量 OSC/BEL 扫描器（每个终端会话一个实例）。
/// 只匹配 ASCII 控制字节（ESC/BEL/]/\），多字节 UTF-8 内容不会误触发。
#[derive(Default)]
struct NotifyScanner {
    /// 0=普通 1=见到 ESC 2=OSC 中 3=OSC 中见到 ESC（期待 ST）
    state: u8,
    buf: Vec<u8>,
    /// OSC 133 C 记录的命令开始时间，D 结算耗时
    cmd_start: Option<std::time::Instant>,
    last_bell: Option<std::time::Instant>,
}

impl NotifyScanner {
    /// 喂入一个输出块，返回捕获到的 (kind, title, body)
    fn feed(&mut self, chunk: &[u8]) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        for &b in chunk {
            match self.state {
                0 => match b {
                    0x07 => self.bell(&mut out),
                    0x1b => self.state = 1,
                    _ => {}
                },
                1 => match b {
                    b']' => {
                        self.state = 2;
                        self.buf.clear();
                    }
                    0x1b => {}
                    0x07 => {
                        self.state = 0;
                        self.bell(&mut out);
                    }
                    _ => self.state = 0,
                },
                2 => match b {
                    0x07 => {
                        self.state = 0;
                        self.finish_osc(&mut out);
                    }
                    0x1b => self.state = 3,
                    _ => {
                        // 超长 OSC 视为异常序列，丢弃防内存膨胀
                        if self.buf.len() < 4096 {
                            self.buf.push(b);
                        } else {
                            self.state = 0;
                            self.buf.clear();
                        }
                    }
                },
                _ => match b {
                    b'\\' => {
                        self.state = 0;
                        self.finish_osc(&mut out);
                    }
                    0x1b => self.state = 1,
                    _ => self.state = 0,
                },
            }
        }
        out
    }

    fn bell(&mut self, out: &mut Vec<(String, String, String)>) {
        let now = std::time::Instant::now();
        if let Some(t) = self.last_bell {
            if now.duration_since(t) < std::time::Duration::from_millis(BELL_DEDUPE_MS as u64) {
                return;
            }
        }
        self.last_bell = Some(now);
        out.push((
            "bell".into(),
            "响铃提醒".into(),
            "终端发出响铃（可能有任务等待处理）".into(),
        ));
    }

    fn finish_osc(&mut self, out: &mut Vec<(String, String, String)>) {
        let content = String::from_utf8_lossy(&self.buf).to_string();
        self.buf.clear();
        let Some((ps, pt)) = content.split_once(';') else {
            return;
        };
        match ps {
            // iTerm2/kitty 通知；9;4;… 是 ConEmu 进度序列，不是通知文本，过滤
            "9" => {
                if pt.starts_with("4;") {
                    return;
                }
                let body = pt.trim();
                if !body.is_empty() {
                    out.push(("notification".into(), "任务通知".into(), body.into()));
                }
            }
            // OSC 777 ; notify ; title ; body
            "777" => {
                let rest = pt.strip_prefix("notify;").unwrap_or(pt);
                let (title, body) = rest.split_once(';').unwrap_or(("任务通知", rest));
                let body = body.trim();
                if !body.is_empty() {
                    out.push(("notification".into(), title.to_string(), body.into()));
                }
            }
            // OSC 133 命令生命周期（shell integration 注入后对任意命令生效）
            "133" => {
                if pt == "C" {
                    self.cmd_start = Some(std::time::Instant::now());
                } else if let Some(code) = pt.strip_prefix("D;") {
                    let exit_code: i32 = code.parse().unwrap_or(0);
                    // 没有配对的 C（如会话中途接入）时跳过
                    if let Some(start) = self.cmd_start.take() {
                        let secs = start.elapsed().as_secs();
                        let failed = exit_code != 0;
                        if failed || secs as u128 * 1000 >= CMD_NOTIFY_MIN_MS {
                            let dur = if secs >= 60 {
                                format!("{}m{}s", secs / 60, secs % 60)
                            } else {
                                format!("{secs}s")
                            };
                            out.push((
                                "command".into(),
                                if failed {
                                    "命令失败".into()
                                } else {
                                    "命令完成".into()
                                },
                                format!("退出码 {exit_code} · 耗时 {dur}"),
                            ));
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// zsh shell integration 脚本：包装用户 .zshrc/.zprofile，追加 OSC 133（命令生命周期）
/// 和 OSC 7（cwd 上报）钩子。每次启动覆写，返回 ZDOTDIR 目录。
fn write_zsh_integration(app: &AppHandle) -> Option<String> {
    let base = app.path().app_data_dir().ok()?;
    let dir = base.join("shell-integration").join("zsh");
    std::fs::create_dir_all(&dir).ok()?;
    let content = r##"# Trace terminal shell integration (auto-generated, 勿手改)
# 先恢复真实 ZDOTDIR 并加载用户自己的配置
if [[ -n "$TRACE_ORIG_ZDOTDIR" ]]; then
  ZDOTDIR="$TRACE_ORIG_ZDOTDIR"
else
  ZDOTDIR="$HOME"
fi
[[ -f "$ZDOTDIR/.zshrc" ]] && source "$ZDOTDIR/.zshrc"

# OSC 133: A=prompt 开始, C=命令执行, D=命令结束(带退出码); OSC 7: 上报 cwd
__trace_precmd() {
  local __code=$?
  printf '\e]133;D;%d\a' $__code
  printf '\e]7;file://%s%s\a' "${HOST:-localhost}" "$PWD"
  printf '\e]133;A\a'
}
__trace_preexec() { printf '\e]133;C\a' }
autoload -Uz add-zsh-hook
add-zsh-hook precmd __trace_precmd
add-zsh-hook preexec __trace_preexec
"##;
    std::fs::write(dir.join(".zshrc"), content).ok()?;
    // login shell(-l)下 zsh 在 .zshrc 之前还会读 $ZDOTDIR/.zprofile；
    // ZDOTDIR 被我们改到了 integration 目录，这里包一层转 source 用户自己的
    // ~/.zprofile(brew shellenv / pyenv init 通常都在这里)，否则 PATH 补不上。
    // 注意此处不能恢复 ZDOTDIR：zsh 读取每个启动文件时按当时的 ZDOTDIR 查找，
    // 若提前恢复，后面的包装 .zshrc(注入 OSC 钩子) 就会被跳过
    let profile = r##"# Trace terminal shell integration (auto-generated, 勿手改)
if [[ -n "$TRACE_ORIG_ZDOTDIR" ]]; then
  __TRACE_REAL_ZDOTDIR="$TRACE_ORIG_ZDOTDIR"
else
  __TRACE_REAL_ZDOTDIR="$HOME"
fi
[[ -f "$__TRACE_REAL_ZDOTDIR/.zprofile" ]] && source "$__TRACE_REAL_ZDOTDIR/.zprofile"
"##;
    std::fs::write(dir.join(".zprofile"), profile).ok()?;
    Some(dir.to_string_lossy().to_string())
}

#[tauri::command]
pub fn term_create(
    app: AppHandle,
    state: State<'_, TermManager>,
    cols: u16,
    rows: u16,
    cwd: Option<String>,
) -> Result<TermInfo, String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let cwd = cwd
        .filter(|c| !c.is_empty())
        .or_else(|| std::env::var("HOME").ok())
        .unwrap_or_else(|| ".".to_string());

    let cols = if cols == 0 { 80 } else { cols };
    let rows = if rows == 0 { 24 } else { rows };
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("openpty failed: {e}"))?;

    let mut cmd = CommandBuilder::new(&shell);
    // 以 login shell 启动：GUI 应用从 Finder 启动时环境干净，login shell 才会走
    // /etc/zprofile(path_helper) 和 ~/.zprofile(brew shellenv 等) 把 PATH 补齐
    cmd.arg("-l");
    cmd.cwd(&cwd);
    cmd.env("TERM", "xterm-256color");
    // 声明为 iTerm2 兼容终端：kimi 等 CLI 按 TERM_PROGRAM 探测通知能力,
    // 不设置时 kimi 降级为裸 BEL(丢失通知标题/正文)
    cmd.env("TERM_PROGRAM", "iTerm.app");
    cmd.env("TERM_PROGRAM_VERSION", "3.5.0");

    // zsh：注入 shell integration（OSC 133 命令生命周期 + OSC 7 cwd 上报），
    // 通过 ZDOTDIR 包装用户的 .zshrc，对任意命令生效
    if std::path::Path::new(&shell)
        .file_name()
        .is_some_and(|n| n == "zsh")
    {
        if let Some(dir) = write_zsh_integration(&app) {
            if let Ok(orig) = std::env::var("ZDOTDIR") {
                cmd.env("TRACE_ORIG_ZDOTDIR", orig);
            }
            cmd.env("ZDOTDIR", dir);
        }
    }

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("spawn shell failed: {e}"))?;
    drop(pair.slave);

    let child_pid = child.process_id().unwrap_or(0);
    let id = uuid::Uuid::new_v4().to_string();
    let scrollback: Arc<Mutex<VecDeque<u8>>> = Arc::new(Mutex::new(VecDeque::new()));
    let alive = Arc::new(AtomicBool::new(true));

    // reader 线程：PTY 输出 → scrollback + 事件推送 + 通知捕获（OSC 9/777/133/BEL）
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("clone reader failed: {e}"))?;
    {
        let scrollback = scrollback.clone();
        let event = format!("term-output/{id}");
        let app = app.clone();
        let term_id = id.clone();
        let shell_for_notify = shell.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            // 跨块增量解码：多字节 UTF-8 字符/转义序列被 read 边界切断时,
            // 不完整尾部留到下一块再解码, 避免 from_utf8_lossy 产生 U+FFFD 乱码
            let mut pending: Vec<u8> = Vec::new();
            let mut scanner = NotifyScanner::default();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = &buf[..n];
                        append_scrollback(&scrollback, chunk);
                        for (kind, title, body) in scanner.feed(chunk) {
                            // 带上前台进程名（如 "kimi · 任务通知"），与原视图侧行为一致
                            let fg = foreground_cmd(child_pid, &shell_for_notify);
                            let _ = app.emit(
                                "term-notify",
                                TermNotifyEvent {
                                    id: term_id.clone(),
                                    kind,
                                    title: format!("{fg} · {title}"),
                                    body,
                                },
                            );
                        }
                        pending.extend_from_slice(chunk);
                        let valid_up_to = match std::str::from_utf8(&pending) {
                            Ok(_) => pending.len(),
                            // 末尾是不完整序列：只发完整部分, 尾部留待下一块
                            Err(e) if e.error_len().is_none() => e.valid_up_to(),
                            // 真有非法字节（极少见）：整块 lossy 发出
                            Err(_) => pending.len(),
                        };
                        if valid_up_to > 0 {
                            let _ = app.emit(
                                &event,
                                String::from_utf8_lossy(&pending[..valid_up_to]).to_string(),
                            );
                            pending.drain(..valid_up_to);
                        }
                    }
                    Err(_) => break,
                }
            }
        });
    }

    // wait 线程：子进程退出后标记 alive=false，并发 term-exit 事件让前端关闭面板
    {
        let alive = alive.clone();
        let app = app.clone();
        let exit_event = format!("term-exit/{id}");
        std::thread::spawn(move || {
            let _ = child.wait();
            alive.store(false, Ordering::SeqCst);
            let _ = app.emit(&exit_event, ());
        });
    }

    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("take writer failed: {e}"))?;

    let session = TermSession {
        id: id.clone(),
        writer,
        master: pair.master,
        child_pid,
        shell,
        cwd,
        created_at: chrono::Utc::now().to_rfc3339(),
        scrollback,
        alive,
        cols,
        rows,
    };
    let info = session_info(&session);
    state.sessions.lock().unwrap().insert(id, session);
    Ok(info)
}

#[tauri::command]
pub fn term_write(state: State<'_, TermManager>, id: String, data: String) -> Result<(), String> {
    let mut sessions = state.sessions.lock().unwrap();
    let session = sessions.get_mut(&id).ok_or("terminal not found")?;
    session
        .writer
        .write_all(data.as_bytes())
        .and_then(|_| session.writer.flush())
        .map_err(|e| format!("write failed: {e}"))
}

#[tauri::command]
pub fn term_resize(
    state: State<'_, TermManager>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let mut sessions = state.sessions.lock().unwrap();
    let session = sessions.get_mut(&id).ok_or("terminal not found")?;
    session
        .master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("resize failed: {e}"))?;
    session.cols = cols;
    session.rows = rows;
    Ok(())
}

#[tauri::command]
pub fn term_close(state: State<'_, TermManager>, id: String) -> Result<(), String> {
    let session = state.sessions.lock().unwrap().remove(&id);
    if let Some(session) = session {
        session.alive.store(false, Ordering::SeqCst);
        #[cfg(unix)]
        unsafe {
            libc::kill(session.child_pid as i32, libc::SIGHUP);
        }
    }
    Ok(())
}

#[tauri::command]
pub fn term_read(
    state: State<'_, TermManager>,
    id: String,
    max_bytes: Option<usize>,
    raw: Option<bool>,
) -> Result<String, String> {
    let sessions = state.sessions.lock().unwrap();
    let session = sessions.get(&id).ok_or("terminal not found")?;
    let buf = session.scrollback.lock().unwrap();
    let len = buf.len();
    let start = match max_bytes {
        Some(max) if max < len => len - max,
        _ => 0,
    };
    let tail: Vec<u8> = buf.iter().skip(start).copied().collect();
    drop(buf);
    // raw=true 保留 ANSI（供 xterm 回放渲染）；默认剥离供纯文本消费
    if raw.unwrap_or(false) {
        return Ok(String::from_utf8_lossy(&tail).to_string());
    }
    let stripped = strip_ansi_escapes::strip(&tail);
    Ok(String::from_utf8_lossy(&stripped).to_string())
}

#[cfg(not(target_os = "macos"))]
fn foreground_cwd(s: &TermSession) -> String {
    s.cwd.clone()
}

#[tauri::command]
pub fn term_foreground_cwd(state: State<'_, TermManager>, id: String) -> Result<String, String> {
    let sessions = state.sessions.lock().unwrap();
    let session = sessions.get(&id).ok_or("terminal not found")?;
    Ok(foreground_cwd(session))
}

#[tauri::command]
pub fn term_list(state: State<'_, TermManager>) -> Vec<TermInfo> {
    let sessions = state.sessions.lock().unwrap();
    sessions.values().map(session_info).collect()
}
