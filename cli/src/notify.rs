//! 终端通知捕获与上报：OSC 9/777/133 与 BEL 扫描器（纯字节状态机），
//! 捕获结果经 mpsc 通道由后台任务 POST /api/client/notify 上报 server。
//! 扫描器迁移自 client/src-tauri/src/terminal.rs，无 Tauri 依赖。

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::api::ApiClient;

/// 命令耗时超过该值才通知（失败命令不受限）
const CMD_NOTIFY_MIN_MS: u128 = 30_000;
/// BEL 响铃去重窗口
const BELL_DEDUPE_MS: u128 = 10_000;

/// 一条捕获到的终端通知（terminal reader 线程 → 上报任务）
pub struct NotifyEvent {
    pub term_id: String,
    pub kind: String,
    pub title: String,
    pub body: String,
}

/// 通知上报通道的发送端，注入各终端会话的 reader 线程
pub type NotifyTx = mpsc::UnboundedSender<NotifyEvent>;

/// 消费通知事件并上报 server；失败只记日志，不影响主流程
pub async fn run(api: Arc<ApiClient>, client_id: String, mut rx: mpsc::UnboundedReceiver<NotifyEvent>) {
    while let Some(ev) = rx.recv().await {
        if let Err(e) = api
            .post_notify(&client_id, Some(&ev.term_id), &ev.kind, &ev.title, &ev.body)
            .await
        {
            tracing::warn!(term_id = %ev.term_id, "上报终端通知失败: {e}");
        }
    }
}

/// PTY 字节流上的增量 OSC/BEL 扫描器（每个终端会话一个实例）。
/// 只匹配 ASCII 控制字节（ESC/BEL/]/\），多字节 UTF-8 内容不会误触发。
#[derive(Default)]
pub struct NotifyScanner {
    /// 0=普通 1=见到 ESC 2=OSC 中 3=OSC 中见到 ESC（期待 ST）
    state: u8,
    buf: Vec<u8>,
    /// OSC 133 C 记录的命令开始时间，D 结算耗时
    cmd_start: Option<std::time::Instant>,
    last_bell: Option<std::time::Instant>,
}

impl NotifyScanner {
    /// 喂入一个输出块，返回捕获到的 (kind, title, body)
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<(String, String, String)> {
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
        let Some((ps, pt)) = content.split_once(';') else { return };
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
                                if failed { "命令失败".into() } else { "命令完成".into() },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_osc9_notification() {
        let mut s = NotifyScanner::default();
        let ev = s.feed(b"\x1b]9;\xe6\xb5\x8b\xe8\xaf\x95\xe9\x80\x9a\xe7\x9f\xa5\x07");
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].0, "notification");
        assert_eq!(ev[0].2, "测试通知");
        // 空正文不触发
        assert!(s.feed(b"\x1b]9;\x07").is_empty());
        // ConEmu 进度序列 9;4;… 不是通知
        assert!(s.feed(b"\x1b]9;4;3;0\x07").is_empty());
    }

    #[test]
    fn captures_osc777_notification() {
        let mut s = NotifyScanner::default();
        let ev = s.feed("\x1b]777;notify;标题;正文\x07".as_bytes());
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].1, "标题");
        assert_eq!(ev[0].2, "正文");
    }

    #[test]
    fn bell_dedupes_within_window() {
        let mut s = NotifyScanner::default();
        assert_eq!(s.feed(b"\x07").len(), 1);
        // 去重窗口内第二个 BEL 被吞
        assert!(s.feed(b"\x07").is_empty());
    }

    #[test]
    fn osc133_failed_command_notifies() {
        let mut s = NotifyScanner::default();
        assert!(s.feed(b"\x1b]133;C\x07").is_empty());
        // 失败命令（退出码非 0）立即通知
        let ev = s.feed(b"\x1b]133;D;1\x07");
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].0, "command");
        assert_eq!(ev[0].1, "命令失败");
        // 快速成功的命令不通知（< 30s）
        assert!(s.feed(b"\x1b]133;C\x07").is_empty());
        assert!(s.feed(b"\x1b]133;D;0\x07").is_empty());
    }

    #[test]
    fn split_sequence_across_chunks() {
        let mut s = NotifyScanner::default();
        // OSC 序列被两块切断也能捕获（增量状态机）
        assert!(s.feed("\x1b]9;跨块".as_bytes()).is_empty());
        let ev = s.feed("通知\x07".as_bytes());
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].2, "跨块通知");
    }
}
