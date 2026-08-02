//! 内置终端：基于 portable-pty 的 PTY 会话管理。
//! 每个会话持有 shell 子进程、写入端、滚动缓冲（256KB 环形）；
//! 输出流上的通知（OSC 9/777/133/BEL）经 mpsc 通道发给上报任务。
//! 迁移自 client/src-tauri/src/terminal.rs（去 Tauri 化：无前端事件推送，
//! scrollback 已够 terminal_read 消费；server 不下发 terminal_resize，故不含 resize）。

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;

use crate::notify::{NotifyEvent, NotifyScanner, NotifyTx};

const SCROLLBACK_CAP: usize = 256 * 1024;

pub struct TermSession {
    pub id: String,
    pub writer: Box<dyn Write + Send>,
    #[allow(dead_code)]
    pub master: Box<dyn MasterPty + Send>,
    pub child_pid: u32,
    pub shell: String,
    pub cwd: String,
    pub created_at: String,
    pub scrollback: Arc<Mutex<VecDeque<u8>>>,
    pub alive: Arc<AtomicBool>,
    /// 是否启用；停用后拒绝写入、不再上报通知（不杀进程）
    pub enabled: Arc<AtomicBool>,
    /// 最后工作时间：最近一次有 PTY 输出或写入的时刻
    pub last_active_at: Arc<Mutex<DateTime<Utc>>>,
    /// 最后在线时间：最近一次被观测到 alive 的时刻（term_list 时刷新）
    pub last_seen_at: Arc<Mutex<DateTime<Utc>>>,
    /// PTY 尺寸，创建时确定（协议无 terminal_resize，运行期不变）
    pub cols: u16,
    pub rows: u16,
}

pub struct TermManager {
    pub sessions: Mutex<HashMap<String, TermSession>>,
    /// 数据目录（~/.hank-cli），zsh shell-integration 写在其下
    data_dir: PathBuf,
}

#[derive(Serialize, Clone, Debug)]
pub struct TermInfo {
    pub id: String,
    pub shell: String,
    pub cwd: String,
    pub foreground_cmd: String,
    pub alive: bool,
    pub created_at: String,
    pub cols: u16,
    pub rows: u16,
    pub enabled: bool,
    pub last_active_at: String,
    pub last_seen_at: String,
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

#[cfg(not(target_os = "macos"))]
fn foreground_cwd(s: &TermSession) -> String {
    s.cwd.clone()
}

fn session_info(s: &TermSession) -> TermInfo {
    let alive = s.alive.load(Ordering::SeqCst);
    // term_list / 观测时刷新 last_seen_at；子进程已退出则冻结在死亡前最后一次观测
    if alive {
        *s.last_seen_at.lock().unwrap() = Utc::now();
    }
    TermInfo {
        id: s.id.clone(),
        shell: s.shell.clone(),
        // 展示用实时 cwd（前台进程 cd 过也能跟上），代价是每次一次 lsof，量级可忽略
        cwd: foreground_cwd(s),
        foreground_cmd: foreground_cmd(s.child_pid, &s.shell),
        alive,
        created_at: s.created_at.clone(),
        cols: s.cols,
        rows: s.rows,
        enabled: s.enabled.load(Ordering::SeqCst),
        last_active_at: s.last_active_at.lock().unwrap().to_rfc3339(),
        last_seen_at: s.last_seen_at.lock().unwrap().to_rfc3339(),
    }
}

fn append_scrollback(buf: &Arc<Mutex<VecDeque<u8>>>, data: &[u8]) {
    let mut b = buf.lock().unwrap();
    b.extend(data);
    while b.len() > SCROLLBACK_CAP {
        b.pop_front();
    }
}

/// zsh shell integration 脚本：包装用户 .zshrc/.zprofile，追加 OSC 133（命令生命周期）
/// 和 OSC 7（cwd 上报）钩子。每次启动覆写，返回 ZDOTDIR 目录。
fn write_zsh_integration(data_dir: &std::path::Path) -> Option<String> {
    let dir = data_dir.join("shell-integration").join("zsh");
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

impl TermManager {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            data_dir,
        }
    }

    pub fn term_create(
        &self,
        cols: u16,
        rows: u16,
        cwd: Option<String>,
        notify_tx: NotifyTx,
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
        // 以 login shell 启动：headless 进程环境可能不干净，login shell 才会走
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
            if let Some(dir) = write_zsh_integration(&self.data_dir) {
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
        let enabled = Arc::new(AtomicBool::new(true));
        let now = Utc::now();
        let last_active_at = Arc::new(Mutex::new(now));
        let last_seen_at = Arc::new(Mutex::new(now));

        // reader 线程：PTY 输出 → scrollback + 通知捕获（OSC 9/777/133/BEL）
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("clone reader failed: {e}"))?;
        {
            let scrollback = scrollback.clone();
            let term_id = id.clone();
            let shell_for_notify = shell.clone();
            let enabled = enabled.clone();
            let last_active_at = last_active_at.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                let mut scanner = NotifyScanner::default();
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let chunk = &buf[..n];
                            append_scrollback(&scrollback, chunk);
                            *last_active_at.lock().unwrap() = Utc::now();
                            // 停用时仍 feed 保持状态机连续，只是不发送通知
                            for (kind, title, body) in scanner.feed(chunk) {
                                if !enabled.load(Ordering::SeqCst) {
                                    continue;
                                }
                                // 带上前台进程名（如 "kimi · 任务通知"），与 app 侧行为一致
                                let fg = foreground_cmd(child_pid, &shell_for_notify);
                                let _ = notify_tx.send(NotifyEvent {
                                    term_id: term_id.clone(),
                                    kind,
                                    title: format!("{fg} · {title}"),
                                    body,
                                });
                            }
                        }
                        Err(_) => break,
                    }
                }
            });
        }

        // wait 线程：子进程退出后标记 alive=false
        {
            let alive = alive.clone();
            std::thread::spawn(move || {
                let _ = child.wait();
                alive.store(false, Ordering::SeqCst);
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
            created_at: now.to_rfc3339(),
            scrollback,
            alive,
            enabled,
            last_active_at,
            last_seen_at,
            cols,
            rows,
        };
        let info = session_info(&session);
        self.sessions.lock().unwrap().insert(id, session);
        Ok(info)
    }

    pub fn term_write(&self, id: &str, data: &str) -> Result<(), String> {
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions.get_mut(id).ok_or("terminal not found")?;
        if !session.enabled.load(Ordering::SeqCst) {
            return Err("terminal disabled".into());
        }
        session
            .writer
            .write_all(data.as_bytes())
            .and_then(|_| session.writer.flush())
            .map_err(|e| format!("write failed: {e}"))?;
        *session.last_active_at.lock().unwrap() = Utc::now();
        Ok(())
    }

    /// 停用/启用会话；返回更新后的会话信息
    pub fn term_set_enabled(&self, id: &str, enabled: bool) -> Result<TermInfo, String> {
        let sessions = self.sessions.lock().unwrap();
        let session = sessions.get(id).ok_or("terminal not found")?;
        session.enabled.store(enabled, Ordering::SeqCst);
        Ok(session_info(session))
    }

    pub fn term_close(&self, id: &str) -> Result<(), String> {
        let session = self.sessions.lock().unwrap().remove(id);
        if let Some(session) = session {
            session.alive.store(false, Ordering::SeqCst);
            #[cfg(unix)]
            unsafe {
                libc::kill(session.child_pid as i32, libc::SIGHUP);
            }
        }
        Ok(())
    }

    pub fn term_read(
        &self,
        id: &str,
        max_bytes: Option<usize>,
        raw: Option<bool>,
    ) -> Result<String, String> {
        let sessions = self.sessions.lock().unwrap();
        let session = sessions.get(id).ok_or("terminal not found")?;
        let buf = session.scrollback.lock().unwrap();
        let len = buf.len();
        let start = match max_bytes {
            Some(max) if max < len => len - max,
            _ => 0,
        };
        let tail: Vec<u8> = buf.iter().skip(start).copied().collect();
        drop(buf);
        // raw=true 保留 ANSI（供 xterm 回放渲染）；默认剥离供纯文本消费。
        // app 侧 raw 优先取 xterm 屏幕快照，CLI 无 xterm，直接走 PTY 原始流
        if raw.unwrap_or(false) {
            return Ok(String::from_utf8_lossy(&tail).to_string());
        }
        let stripped = strip_ansi_escapes::strip(&tail);
        Ok(String::from_utf8_lossy(&stripped).to_string())
    }

    pub fn term_list(&self) -> Vec<TermInfo> {
        let sessions = self.sessions.lock().unwrap();
        sessions.values().map(session_info).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_manager(name: &str) -> TermManager {
        TermManager::new(std::env::temp_dir().join(format!("hank-cli-test-{name}")))
    }

    fn notify_tx() -> (NotifyTx, tokio::sync::mpsc::UnboundedReceiver<NotifyEvent>) {
        tokio::sync::mpsc::unbounded_channel()
    }

    /// 等输出进 scrollback（reader 线程异步写）
    fn wait_output(mgr: &TermManager, id: &str, needle: &str) -> String {
        for _ in 0..50 {
            let out = mgr.term_read(id, None, None).unwrap_or_default();
            if out.contains(needle) {
                return out;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        mgr.term_read(id, None, None).unwrap_or_default()
    }

    #[test]
    fn create_write_read_list_close() {
        let mgr = test_manager("term-basic");
        let (tx, _rx) = notify_tx();
        let info = mgr.term_create(120, 30, None, tx).unwrap();
        assert_eq!(info.cols, 120);
        assert_eq!(info.rows, 30);
        assert!(info.alive);
        assert!(!info.shell.is_empty());

        mgr.term_write(&info.id, "echo hank-cli-test-ok\n").unwrap();
        let out = wait_output(&mgr, &info.id, "hank-cli-test-ok");
        assert!(out.contains("hank-cli-test-ok"), "output: {out}");

        let list = mgr.term_list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, info.id);

        mgr.term_close(&info.id).unwrap();
        assert!(mgr.term_list().is_empty());
        // 关闭后再写/读报错
        assert!(mgr.term_write(&info.id, "x").is_err());
        assert!(mgr.term_read(&info.id, None, None).is_err());
    }

    #[test]
    fn read_max_bytes_and_raw() {
        let mgr = test_manager("term-read");
        let (tx, _rx) = notify_tx();
        let info = mgr.term_create(80, 24, None, tx).unwrap();
        mgr.term_write(&info.id, "echo raw-marker\n").unwrap();
        wait_output(&mgr, &info.id, "raw-marker");

        // max_bytes 截尾：只取最后 16 字节
        let tail = mgr.term_read(&info.id, Some(16), None).unwrap();
        assert!(tail.len() <= 16, "tail len {} > 16", tail.len());

        // raw 保留 ANSI 转义（shell 提示符一般会带），非 raw 不含 ESC
        let plain = mgr.term_read(&info.id, None, None).unwrap();
        assert!(
            !plain.contains('\x1b'),
            "plain output still has ESC: {plain:?}"
        );

        mgr.term_close(&info.id).unwrap();
    }

    #[test]
    fn create_with_zero_size_falls_back() {
        let mgr = test_manager("term-zero");
        let (tx, _rx) = notify_tx();
        // 0 尺寸回退 80x24
        let info = mgr.term_create(0, 0, None, tx).unwrap();
        assert_eq!(info.cols, 80);
        assert_eq!(info.rows, 24);
        mgr.term_close(&info.id).unwrap();
    }

    #[test]
    fn set_enabled_blocks_write_but_keeps_read() {
        let mgr = test_manager("term-enabled");
        let (tx, _rx) = notify_tx();
        let info = mgr.term_create(80, 24, None, tx).unwrap();
        assert!(info.enabled);

        mgr.term_write(&info.id, "echo enable-marker\n").unwrap();
        let out = wait_output(&mgr, &info.id, "enable-marker");
        assert!(out.contains("enable-marker"), "output: {out}");

        let disabled = mgr.term_set_enabled(&info.id, false).unwrap();
        assert!(!disabled.enabled);
        assert!(mgr.term_write(&info.id, "echo should-fail\n").is_err());

        // 读与列表不受停用影响
        let still = mgr.term_read(&info.id, None, None).unwrap();
        assert!(still.contains("enable-marker"));
        let list = mgr.term_list();
        let found = list.iter().find(|t| t.id == info.id).unwrap();
        assert!(!found.enabled);

        // 启用后写入恢复
        let enabled = mgr.term_set_enabled(&info.id, true).unwrap();
        assert!(enabled.enabled);
        mgr.term_write(&info.id, "echo re-enabled\n").unwrap();
        let out = wait_output(&mgr, &info.id, "re-enabled");
        assert!(out.contains("re-enabled"), "output: {out}");

        mgr.term_close(&info.id).unwrap();
    }

    #[test]
    fn last_active_at_advances_on_write() {
        let mgr = test_manager("term-active");
        let (tx, _rx) = notify_tx();
        let info = mgr.term_create(80, 24, None, tx).unwrap();
        let before = DateTime::parse_from_rfc3339(&info.last_active_at)
            .unwrap()
            .with_timezone(&Utc);

        // 稍等再写，避免同一秒内时间戳完全相同
        std::thread::sleep(std::time::Duration::from_millis(50));
        mgr.term_write(&info.id, "echo active-marker\n").unwrap();
        wait_output(&mgr, &info.id, "active-marker");

        let list = mgr.term_list();
        let found = list.iter().find(|t| t.id == info.id).unwrap();
        let after = DateTime::parse_from_rfc3339(&found.last_active_at)
            .unwrap()
            .with_timezone(&Utc);
        assert!(
            after > before,
            "last_active_at should advance: before={before} after={after}"
        );

        mgr.term_close(&info.id).unwrap();
    }

    #[test]
    fn term_set_enabled_unknown_id_errors() {
        let mgr = test_manager("term-unknown-enabled");
        let err = mgr.term_set_enabled("no-such-id", false).unwrap_err();
        assert_eq!(err, "terminal not found");
    }
}
