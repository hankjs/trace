//! VT 屏幕缓冲：PTY 字节流 → 权威屏幕状态。
//!
//! 为什么放在 CLI 侧：VT 屏幕是对**完整有序字节流**的折叠结果，丢一段或乱一次序，
//! 屏幕就永久错位且无法自愈（除非 TUI 恰好整屏重绘）。handy-cli 是链路上唯一
//! 能保证看到每个 PTY 字节且有序的组件，解析器与 reader 线程同处一条链上，
//! 没有丢字节的可能。下游（handy / web）因此不需要任何 vt 解析能力。
//!
//! 对外给两路输出，覆盖两类消费者：
//! - `snapshot()`：权威屏幕快照（纯文本）。一屏 120×30 约 3.6KB，LLM 与无头决策
//!   读这个，不依赖浏览器在线。
//! - `deltas_since(seq)`：带序号的原始字节增量，xterm.js 回放用（颜色/超链接保真
//!   最好）。序号有缺口时改取 `ansi_snapshot()` 整屏对齐。

use std::collections::VecDeque;

use serde::{Serialize, Serializer};
use vt100::Parser;

/// 原始增量环形缓冲上限：够浏览器断线几十秒后续播，超出则让它整屏重取
const DELTA_CAP_BYTES: usize = 256 * 1024;
/// 单个增量分片上限：避免一次大块输出变成一条巨帧
const DELTA_CHUNK_BYTES: usize = 16 * 1024;
/// vt100 回滚行数（滚出屏幕的历史输出）
const SCROLLBACK_LINES: usize = 2000;

/// 一段带序号的原始字节增量。`seq` 单调递增且连续，消费者据此判断有无缺口。
#[derive(Clone, Serialize)]
pub struct Delta {
    pub seq: u64,
    /// 原始 PTY 字节（含 ANSI）。序列化为 base64：字节流不保证落在 UTF-8 边界上，
    /// 直接塞进 JSON 字符串会被 lossy 替换破坏转义序列。
    #[serde(rename = "data_b64", serialize_with = "serialize_base64")]
    pub data: Vec<u8>,
}

fn serialize_base64<S: Serializer>(data: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
    use base64::Engine;
    serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(data))
}

/// 屏幕快照。`seq` 是生成快照时已消费到的增量序号，消费者拿它作为后续
/// `deltas_since` 的起点，保证快照与增量无缝衔接。
#[allow(dead_code)]
#[derive(Clone, Serialize)]
pub struct Snapshot {
    pub seq: u64,
    pub cols: u16,
    pub rows: u16,
    /// 光标位置（0-based）
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub cursor_visible: bool,
    /// 纯文本屏幕（行间 \n，行尾空格已裁剪）
    pub text: String,
    /// 应用是否在备用屏幕（全屏 TUI 通常为 true）
    pub alternate_screen: bool,
    /// 窗口标题（TUI 常用它显示状态；经 OSC 1/2 回调捕获）
    pub title: String,
}

/// vt100 回调：只捕获窗口标题。
///
/// 通知（OSC 9/777/133、BEL）仍由 `notify::NotifyScanner` 在字节层扫描 —— 它需要
/// OSC 133 的命令计时与 BEL 去重逻辑，vt100 的回调拿不到这些语义。
#[derive(Default)]
struct ScreenCallbacks {
    title: String,
}

impl vt100::Callbacks for ScreenCallbacks {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = String::from_utf8_lossy(title).to_string();
    }
}

/// PTY 字节流的屏幕状态机 + 原始增量缓冲。
///
/// 由 PTY reader 线程独占（外层 Mutex 包裹），`feed` 必须按字节到达顺序调用。
pub struct ScreenBuffer {
    parser: Parser<ScreenCallbacks>,
    /// 已生成的增量分片，按 seq 递增
    deltas: VecDeque<Delta>,
    /// deltas 中原始字节总量
    delta_bytes: usize,
    /// 下一个分片的序号
    next_seq: u64,
}

impl ScreenBuffer {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: Parser::new_with_callbacks(
                rows,
                cols,
                SCROLLBACK_LINES,
                ScreenCallbacks::default(),
            ),
            deltas: VecDeque::new(),
            delta_bytes: 0,
            next_seq: 1,
        }
    }

    /// 喂入一块 PTY 输出：同步更新屏幕状态，并切片存入增量缓冲。
    /// 返回本次产生的分片序号区间（含首含尾），空输入返回 None。
    pub fn feed(&mut self, chunk: &[u8]) -> Option<(u64, u64)> {
        if chunk.is_empty() {
            return None;
        }
        // 屏幕状态先更新：即使增量后续被淘汰，快照始终权威
        self.parser.process(chunk);

        let first = self.next_seq;
        for piece in chunk.chunks(DELTA_CHUNK_BYTES) {
            self.deltas.push_back(Delta {
                seq: self.next_seq,
                data: piece.to_vec(),
            });
            self.next_seq += 1;
            self.delta_bytes += piece.len();
        }
        // 超限淘汰最旧分片：消费者发现起点缺失时整屏重取
        while self.delta_bytes > DELTA_CAP_BYTES {
            match self.deltas.pop_front() {
                Some(dropped) => self.delta_bytes -= dropped.data.len(),
                None => {
                    self.delta_bytes = 0;
                    break;
                }
            }
        }
        Some((first, self.next_seq - 1))
    }

    /// 调整屏幕尺寸。必须与 PTY resize 同步调用，否则屏幕状态与应用认知错位。
    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows, cols);
    }

    /// 当前已消费到的增量序号
    pub fn seq(&self) -> u64 {
        self.next_seq - 1
    }

    /// 屏幕快照：LLM / 无头决策的主要输入
    #[allow(dead_code)]
    pub fn snapshot(&self) -> Snapshot {
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        let (cursor_row, cursor_col) = screen.cursor_position();
        Snapshot {
            seq: self.seq(),
            cols,
            rows,
            cursor_row,
            cursor_col,
            cursor_visible: !screen.hide_cursor(),
            text: visible_text(screen, cols),
            alternate_screen: screen.alternate_screen(),
            title: self.parser.callbacks().title.clone(),
        }
    }

    /// 带 ANSI 的整屏内容：xterm.js 整屏对齐用。
    /// vt100 已在其中带上清屏、属性复位与光标归位，可直接 write 进终端。
    pub fn ansi_snapshot(&self) -> Vec<u8> {
        self.parser.screen().contents_formatted()
    }

    /// 回滚历史 + 当前屏的纯文本，取末尾 `max_lines` 行（0 表示不限）。
    ///
    /// vt100 的 `visible_rows` 一次只给一屏，要读历史必须移动 scrollback 偏移，
    /// 因此需要 `&mut self`；读完恢复偏移到 0，不影响 `snapshot()`。
    #[allow(dead_code)]
    pub fn history_text(&mut self, max_lines: usize) -> String {
        let (rows, cols) = self.parser.screen().size();
        let rows = usize::from(rows);
        let screen = self.parser.screen_mut();

        // set_scrollback 会 clamp 到实际历史长度，借此探出最大偏移
        screen.set_scrollback(usize::MAX);
        let max_offset = screen.scrollback();

        // 从最旧一屏往新走，按屏拼接；偏移不是 rows 整数倍时末屏会与前一屏重叠，
        // 用绝对行号裁掉重复部分
        let mut all: Vec<String> = Vec::new();
        let mut offset = max_offset;
        loop {
            screen.set_scrollback(offset);
            let abs_start = max_offset - offset;
            let view: Vec<String> = screen.rows(0, cols).collect();
            if abs_start + view.len() > all.len() {
                let skip = all.len().saturating_sub(abs_start);
                all.extend(view[skip..].iter().cloned());
            }
            if offset == 0 {
                break;
            }
            offset = offset.saturating_sub(rows);
        }
        screen.set_scrollback(0);

        // 裁掉尾部空行：光标所在的空行不是内容，否则 max_lines 会被它占额度
        while all.last().is_some_and(|line| line.trim_end().is_empty()) {
            all.pop();
        }

        let start = if max_lines == 0 {
            0
        } else {
            all.len().saturating_sub(max_lines)
        };
        all[start..]
            .iter()
            .map(|line| line.trim_end())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 取 `after_seq` 之后的增量。第二个返回值为 false 表示起点已被淘汰
    /// （存在缺口），消费者应改取 `ansi_snapshot()` 整屏对齐。
    pub fn deltas_since(&self, after_seq: u64) -> (Vec<Delta>, bool) {
        // 缓冲为空说明消费者已追平，无缺口；否则要求 after_seq 至少接上最旧分片
        let contiguous = match self.deltas.front().map(|d| d.seq) {
            Some(oldest) => after_seq + 1 >= oldest,
            None => true,
        };
        let items = self
            .deltas
            .iter()
            .filter(|d| d.seq > after_seq)
            .cloned()
            .collect();
        (items, contiguous)
    }
}

/// 逐行取可见屏幕文本并裁掉行尾空格：VT 屏幕是定宽矩阵，不裁剪会让每行补满空格，
/// 传输量翻倍且对 LLM 全是噪声。
#[allow(dead_code)]
fn visible_text(screen: &vt100::Screen, cols: u16) -> String {
    screen
        .rows(0, cols)
        .map(|line| line.trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 屏幕快照按非空行比较：尾部空行对断言无意义
    fn lines(buffer: &ScreenBuffer) -> Vec<String> {
        buffer
            .snapshot()
            .text
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn folds_plain_output_into_screen() {
        let mut buffer = ScreenBuffer::new(24, 80);
        buffer.feed(b"hello\r\nworld\r\n");
        assert_eq!(lines(&buffer), vec!["hello", "world"]);
    }

    /// 这条是整个设计的动机：TUI 用光标移动 + 行内重绘更新界面，
    /// 原始字节流读出来是转义序列噪声，只有折叠成屏幕才能读懂。
    #[test]
    fn folds_cursor_addressing_into_final_screen() {
        let mut buffer = ScreenBuffer::new(10, 40);
        // 写三行，再用 CUP 回到第 2 行行首覆写
        buffer.feed(b"line-a\r\nline-b\r\nline-c\r\n");
        buffer.feed(b"\x1b[2;1Hrewritten");
        let text = lines(&buffer);
        assert_eq!(text, vec!["line-a", "rewritten", "line-c"]);
    }

    /// 清屏 + 重绘后，屏幕只应剩新内容（不是历史累积）
    #[test]
    fn clear_screen_resets_visible_text() {
        let mut buffer = ScreenBuffer::new(10, 40);
        buffer.feed(b"stale content\r\n");
        buffer.feed(b"\x1b[2J\x1b[H");
        buffer.feed(b"fresh");
        assert_eq!(lines(&buffer), vec!["fresh"]);
    }

    #[test]
    fn tracks_cursor_and_alternate_screen() {
        let mut buffer = ScreenBuffer::new(24, 80);
        // 进备用屏幕（全屏 TUI 的典型开场）
        buffer.feed(b"\x1b[?1049h");
        buffer.feed(b"\x1b[5;10H");
        let snapshot = buffer.snapshot();
        assert!(snapshot.alternate_screen);
        assert_eq!((snapshot.cursor_row, snapshot.cursor_col), (4, 9));

        // 隐藏光标
        buffer.feed(b"\x1b[?25l");
        assert!(!buffer.snapshot().cursor_visible);

        // 退出备用屏幕
        buffer.feed(b"\x1b[?1049l");
        assert!(!buffer.snapshot().alternate_screen);
    }

    #[test]
    fn captures_window_title() {
        let mut buffer = ScreenBuffer::new(24, 80);
        buffer.feed(b"\x1b]2;claude \xe2\x80\x94 running\x07");
        assert_eq!(buffer.snapshot().title, "claude — running");
    }

    #[test]
    fn wide_chars_and_split_sequences_survive_chunking() {
        let mut buffer = ScreenBuffer::new(10, 40);
        // UTF-8 多字节被切成两块：vt100 内部缓冲应跨块拼接
        buffer.feed("中文".as_bytes());
        let bytes = "测试".as_bytes();
        buffer.feed(&bytes[..3]);
        buffer.feed(&bytes[3..]);
        assert_eq!(lines(&buffer), vec!["中文测试"]);

        // 转义序列被切断也应正确解析
        let mut buffer = ScreenBuffer::new(10, 40);
        buffer.feed(b"abc\x1b[");
        buffer.feed(b"2;1Hxyz");
        assert_eq!(lines(&buffer), vec!["abc", "xyz"]);
    }

    #[test]
    fn deltas_are_contiguous_and_resumable() {
        let mut buffer = ScreenBuffer::new(24, 80);
        assert!(buffer.feed(b"").is_none());
        let (first, last) = buffer.feed(b"one").unwrap();
        assert_eq!((first, last), (1, 1));
        buffer.feed(b"two").unwrap();

        // 从 0 开始拿全部，序号连续
        let (all, contiguous) = buffer.deltas_since(0);
        assert!(contiguous);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].seq, 1);
        assert_eq!(all[0].data, b"one");
        assert_eq!(all[1].data, b"two");

        // 从 seq=1 之后续播只拿到第二段
        let (rest, contiguous) = buffer.deltas_since(1);
        assert!(contiguous);
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].seq, 2);

        // 已追平：无新增量但也无缺口
        let (none, contiguous) = buffer.deltas_since(buffer.seq());
        assert!(contiguous);
        assert!(none.is_empty());
    }

    #[test]
    fn large_output_splits_into_chunks_and_evicts_old() {
        let mut buffer = ScreenBuffer::new(24, 80);
        // 一次写入超过分片上限：应切成多片
        let (first, last) = buffer.feed(&vec![b'x'; DELTA_CHUNK_BYTES * 2 + 1]).unwrap();
        assert_eq!(first, 1);
        assert_eq!(last, 3);

        // 持续写到超过环形上限，最旧分片被淘汰 → 从 0 续播时报缺口
        for _ in 0..40 {
            buffer.feed(&vec![b'y'; DELTA_CHUNK_BYTES]);
        }
        let (_, contiguous) = buffer.deltas_since(0);
        assert!(!contiguous, "应报告缺口，消费者需整屏重取");

        // 但屏幕状态不受增量淘汰影响，仍然权威
        assert!(!buffer.snapshot().text.is_empty());

        // 从缓冲内的较新起点续播仍然连续
        let newest = buffer.seq();
        let (items, contiguous) = buffer.deltas_since(newest - 1);
        assert!(contiguous);
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn resize_reflows_screen() {
        let mut buffer = ScreenBuffer::new(24, 80);
        buffer.feed(b"resized");
        buffer.resize(30, 100);
        let snapshot = buffer.snapshot();
        assert_eq!((snapshot.rows, snapshot.cols), (30, 100));
        assert_eq!(lines(&buffer), vec!["resized"]);
    }

    #[test]
    fn history_text_reaches_beyond_visible_screen() {
        let mut buffer = ScreenBuffer::new(5, 40);
        // 写 20 行，屏幕只剩最后 5 行，其余进回滚区
        for i in 1..=20 {
            buffer.feed(format!("row-{i}\r\n").as_bytes());
        }
        let visible = lines(&buffer);
        assert!(!visible.contains(&"row-1".to_string()));

        let history = buffer.history_text(0);
        let history_lines: Vec<&str> = history.lines().filter(|l| !l.is_empty()).collect();
        // 回滚区能取回被滚出屏幕的早期行，且顺序不乱、不重复
        assert_eq!(history_lines.first(), Some(&"row-1"));
        assert_eq!(history_lines.last(), Some(&"row-20"));
        assert_eq!(history_lines.len(), 20);

        // 限制行数时从末尾取
        let tail = buffer.history_text(3);
        assert_eq!(
            tail.lines().filter(|l| !l.is_empty()).collect::<Vec<_>>(),
            vec!["row-18", "row-19", "row-20"]
        );

        // 读历史不应改变 snapshot 的可见屏幕
        assert_eq!(lines(&buffer), visible);
    }

    #[test]
    fn ansi_snapshot_replays_into_a_fresh_screen() {
        let mut origin = ScreenBuffer::new(10, 40);
        origin.feed(b"\x1b[1;31mred\x1b[0m\r\nplain\r\n");
        origin.feed(b"\x1b[3;1Hthird");

        // 整屏 ANSI 快照喂进空白屏幕，应还原出相同文本（xterm.js 重连即走这条）
        let mut replay = ScreenBuffer::new(10, 40);
        replay.feed(&origin.ansi_snapshot());
        assert_eq!(lines(&replay), lines(&origin));
    }
}
