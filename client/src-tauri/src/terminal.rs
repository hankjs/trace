//! 内置终端：基于 portable-pty 的 PTY 会话管理。
//! 每个会话持有 shell 子进程、写入端、滚动缓冲（256KB 环形），
//! 输出通过 `term-output/{id}` 事件实时推给前端。

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

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
    let output = match std::process::Command::new("ps")
        .args(["-Ao", "pid=,ppid=,comm="])
        .output()
    {
        Ok(o) if o.status.success() => o.stdout,
        _ => return fallback(),
    };
    let text = String::from_utf8_lossy(&output);
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
    let mut deepest = fallback();
    let mut guard = 0;
    while let Some(kids) = children.get(&cur) {
        if kids.is_empty() || guard > 64 {
            break;
        }
        guard += 1;
        let (pid, comm) = &kids[0];
        deepest = std::path::Path::new(comm)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| comm.clone());
        cur = *pid;
    }
    deepest
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
        cwd: s.cwd.clone(),
        foreground_cmd: foreground_cmd(s.child_pid, &s.shell),
        alive: s.alive.load(Ordering::SeqCst),
        created_at: s.created_at.clone(),
    }
}

fn append_scrollback(buf: &Arc<Mutex<VecDeque<u8>>>, data: &[u8]) {
    let mut b = buf.lock().unwrap();
    b.extend(data);
    while b.len() > SCROLLBACK_CAP {
        b.pop_front();
    }
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

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: if rows == 0 { 24 } else { rows },
            cols: if cols == 0 { 80 } else { cols },
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("openpty failed: {e}"))?;

    let mut cmd = CommandBuilder::new(&shell);
    cmd.cwd(&cwd);
    cmd.env("TERM", "xterm-256color");
    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("spawn shell failed: {e}"))?;
    drop(pair.slave);

    let child_pid = child.process_id().unwrap_or(0);
    let id = uuid::Uuid::new_v4().to_string();
    let scrollback: Arc<Mutex<VecDeque<u8>>> = Arc::new(Mutex::new(VecDeque::new()));
    let alive = Arc::new(AtomicBool::new(true));

    // reader 线程：PTY 输出 → scrollback + 事件推送
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("clone reader failed: {e}"))?;
    {
        let scrollback = scrollback.clone();
        let event = format!("term-output/{id}");
        let app = app.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            // 跨块增量解码：多字节 UTF-8 字符/转义序列被 read 边界切断时,
            // 不完整尾部留到下一块再解码, 避免 from_utf8_lossy 产生 U+FFFD 乱码
            let mut pending: Vec<u8> = Vec::new();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = &buf[..n];
                        append_scrollback(&scrollback, chunk);
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
        created_at: chrono::Utc::now().to_rfc3339(),
        scrollback,
        alive,
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
    let sessions = state.sessions.lock().unwrap();
    let session = sessions.get(&id).ok_or("terminal not found")?;
    session
        .master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("resize failed: {e}"))
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
    let stripped = strip_ansi_escapes::strip(&tail);
    Ok(String::from_utf8_lossy(&stripped).to_string())
}

#[tauri::command]
pub fn term_list(state: State<'_, TermManager>) -> Vec<TermInfo> {
    let sessions = state.sessions.lock().unwrap();
    sessions.values().map(session_info).collect()
}
