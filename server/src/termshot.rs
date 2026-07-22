//! 终端屏幕截图渲染：把 client 返回的带 SGR 转义码的屏幕快照渲染成 PNG 图片。
//!
//! 流程：SGR 文本 → 单元格行（字符 + 前景/背景色）→ SVG → resvg(usvg + tiny-skia) 栅格化 PNG。
//! 字体由 fontdb 加载系统字体，可用 `TERMSHOT_FONT` 环境变量指定字体文件路径兜底加载。
//! 注意：部署机需要安装等宽 CJK 字体（如 Noto Sans Mono CJK），否则中文会缺字形（豆腐块）。

use anyhow::{anyhow, bail, Result};

/// 终端颜色：调色板索引（16 色/256 色）或真彩色 RGB
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Color {
    Ansi(u8),
    Rgb(u8, u8, u8),
}

/// 单元格样式，只保留 fg/bg/bold，其余 SGR 属性忽略
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct CellStyle {
    fg: Option<Color>,
    bg: Option<Color>,
    bold: bool,
}

#[derive(Clone, Copy, Debug)]
struct Cell {
    ch: char,
    style: CellStyle,
}

/// 默认前景色（背景固定深色 #1e1e1e）
const DEFAULT_FG: [u8; 3] = [212, 212, 212];

/// xterm 标准 16 色调色板
const ANSI16: [[u8; 3]; 16] = [
    [0, 0, 0],
    [205, 0, 0],
    [0, 205, 0],
    [205, 205, 0],
    [0, 0, 238],
    [205, 0, 205],
    [0, 205, 205],
    [229, 229, 229],
    [127, 127, 127],
    [255, 0, 0],
    [0, 255, 0],
    [255, 255, 0],
    [92, 92, 255],
    [255, 0, 255],
    [0, 255, 255],
    [255, 255, 255],
];

/// 渲染入口：SGR 快照文本 → PNG bytes
pub fn render_png(sgr_text: &str) -> Result<Vec<u8>> {
    let lines = parse(sgr_text);
    if lines.iter().all(|l| l.iter().all(|c| c.ch == ' ')) {
        bail!("终端快照为空");
    }
    let svg = build_svg(&lines);
    let mut opt = usvg::Options::default();
    opt.fontdb_mut().load_system_fonts();
    // 兜底：TERMSHOT_FONT 指定字体文件（部署机建议装等宽 CJK 字体，否则中文缺字形）
    if let Ok(path) = std::env::var("TERMSHOT_FONT") {
        if let Err(e) = opt.fontdb_mut().load_font_file(&path) {
            tracing::warn!("termshot: load TERMSHOT_FONT {path} failed: {e}");
        }
    }
    let tree = usvg::Tree::from_str(&svg, &opt)?;
    let size = tree.size();
    let (w, h) = (size.width().ceil() as u32, size.height().ceil() as u32);
    let mut pixmap =
        tiny_skia::Pixmap::new(w.max(1), h.max(1)).ok_or_else(|| anyhow!("创建画布失败 {w}x{h}"))?;
    resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());
    Ok(pixmap.encode_png()?)
}

/// 去掉 ANSI 转义码的纯文本（截图/发图失败时降级为文本输出用）
pub fn strip_ansi(sgr_text: &str) -> String {
    parse(sgr_text)
        .iter()
        .map(|l| l.iter().map(|c| c.ch).collect::<String>().trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 解析 SGR 快照为单元格行。只认 CSI ... m（SGR），其他转义序列（如 \x1b[K）跳过。
fn parse(text: &str) -> Vec<Vec<Cell>> {
    let mut lines: Vec<Vec<Cell>> = Vec::new();
    let mut line: Vec<Cell> = Vec::new();
    let mut style = CellStyle::default();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    // 收集 CSI 参数直到最终字节（0x40-0x7E）
                    let mut params = String::new();
                    let mut final_byte = '\0';
                    for c in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&c) {
                            final_byte = c;
                            break;
                        }
                        params.push(c);
                    }
                    if final_byte == 'm' {
                        apply_sgr(&mut style, &params);
                    }
                    // 其他 CSI 序列直接忽略
                }
                // 非 CSI 的 ESC 序列忽略
            }
            '\r' => {}
            '\n' => lines.push(std::mem::take(&mut line)),
            c => {
                let width = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
                if width == 0 {
                    continue;
                }
                line.push(Cell { ch: c, style });
                if width == 2 {
                    // 宽字符（CJK）占两列，补一个占位空格
                    line.push(Cell { ch: ' ', style });
                }
            }
        }
    }
    lines.push(line);
    // 去掉末尾空行
    while lines.len() > 1 && lines.last().is_some_and(|l| l.iter().all(|c| c.ch == ' ')) {
        lines.pop();
    }
    lines
}

/// 应用一串 SGR 参数（如 "0;1;38;2;255;0;0"）
fn apply_sgr(style: &mut CellStyle, params: &str) {
    let codes: Vec<u16> = params.split(';').map(|p| p.parse().unwrap_or(0)).collect();
    let mut i = 0;
    while i < codes.len() {
        match codes[i] {
            0 => *style = CellStyle::default(),
            1 => style.bold = true,
            22 => style.bold = false,
            30..=37 => style.fg = Some(Color::Ansi((codes[i] - 30) as u8)),
            38 | 48 => {
                // 扩展颜色：38;5;n（256 色）/ 38;2;r;g;b（RGB），48 同理为背景
                let is_fg = codes[i] == 38;
                let color = match codes.get(i + 1) {
                    Some(5) => {
                        let c = codes.get(i + 2).map(|&n| Color::Ansi(n as u8));
                        i += 2;
                        c
                    }
                    Some(2) => {
                        let r = codes.get(i + 2).copied().unwrap_or(0) as u8;
                        let g = codes.get(i + 3).copied().unwrap_or(0) as u8;
                        let b = codes.get(i + 4).copied().unwrap_or(0) as u8;
                        i += 4;
                        Some(Color::Rgb(r, g, b))
                    }
                    _ => None,
                };
                if let Some(color) = color {
                    if is_fg {
                        style.fg = Some(color);
                    } else {
                        style.bg = Some(color);
                    }
                }
            }
            39 => style.fg = None,
            40..=47 => style.bg = Some(Color::Ansi((codes[i] - 40) as u8)),
            49 => style.bg = None,
            90..=97 => style.fg = Some(Color::Ansi((codes[i] - 90 + 8) as u8)),
            100..=107 => style.bg = Some(Color::Ansi((codes[i] - 100 + 8) as u8)),
            _ => {}
        }
        i += 1;
    }
}

/// xterm 256 色：0-15 标准色，16-231 为 6×6×6 彩色立方，232-255 灰阶
fn color_rgb(c: Color) -> [u8; 3] {
    match c {
        Color::Ansi(n @ 0..=15) => ANSI16[n as usize],
        Color::Ansi(n @ 16..=231) => {
            const LEVEL: [u8; 6] = [0, 95, 135, 175, 215, 255];
            let n = n - 16;
            [
                LEVEL[(n / 36) as usize],
                LEVEL[((n / 6) % 6) as usize],
                LEVEL[(n % 6) as usize],
            ]
        }
        Color::Ansi(n) => {
            let v = 8 + (n - 232) * 10;
            [v, v, v]
        }
        Color::Rgb(r, g, b) => [r, g, b],
    }
}

/// 前景色：bold + 标准色（0-7）时映射到高亮色（8-15）
fn fg_rgb(style: &CellStyle) -> [u8; 3] {
    match style.fg {
        Some(Color::Ansi(n)) if style.bold && n < 8 => ANSI16[(n + 8) as usize],
        Some(c) => color_rgb(c),
        None => DEFAULT_FG,
    }
}

fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// 单元格行 → SVG 文本（深色背景、等宽字体、相同颜色 run 合并 tspan、bg 色 rect 垫底）
fn build_svg(lines: &[Vec<Cell>]) -> String {
    const FONT_SIZE: f32 = 14.0;
    const LINE_H: f32 = 18.0;
    const CHAR_W: f32 = 8.4; // 0.6 * FONT_SIZE，等宽字体单倍宽
    const PAD: f32 = 10.0;
    let cols = lines.iter().map(|l| l.len()).max().unwrap_or(0).max(1);
    let width = PAD * 2.0 + cols as f32 * CHAR_W;
    let height = PAD * 2.0 + lines.len() as f32 * LINE_H;
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width:.0}" height="{height:.0}" viewBox="0 0 {width:.1} {height:.1}" xml:space="preserve">"#
    );
    svg.push_str(r##"<rect width="100%" height="100%" fill="#1e1e1e"/>"##);
    for (row, line) in lines.iter().enumerate() {
        let y = PAD + row as f32 * LINE_H;
        // 背景色块：连续相同 bg 合并为一个 rect
        let mut col = 0;
        while col < line.len() {
            let bg = line[col].style.bg;
            let start = col;
            while col < line.len() && line[col].style.bg == bg {
                col += 1;
            }
            if let Some(bg) = bg {
                svg.push_str(&format!(
                    r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="{}"/>"#,
                    PAD + start as f32 * CHAR_W,
                    y,
                    (col - start) as f32 * CHAR_W,
                    LINE_H,
                    hex(color_rgb(bg))
                ));
            }
        }
        // 文字：连续相同 (fg, bold) 合并为一个 tspan，x 按列绝对定位
        let mut text = String::new();
        let mut col = 0;
        while col < line.len() {
            let style = line[col].style;
            let start = col;
            let mut run = String::new();
            while col < line.len()
                && (line[col].style.fg, line[col].style.bold) == (style.fg, style.bold)
            {
                run.push(line[col].ch);
                col += 1;
            }
            let bold = if style.bold { r#" font-weight="bold""# } else { "" };
            text.push_str(&format!(
                r#"<tspan x="{:.1}" fill="{}"{bold}>{}</tspan>"#,
                PAD + start as f32 * CHAR_W,
                hex(fg_rgb(&style)),
                escape_xml(&run)
            ));
        }
        if !line.is_empty() {
            svg.push_str(&format!(
                r#"<text y="{:.1}" font-family="Menlo, 'Noto Sans Mono CJK SC', 'WenQuanYi Micro Hei Mono', monospace" font-size="{FONT_SIZE}">{text}</text>"#,
                y + FONT_SIZE - 1.0
            ));
        }
    }
    svg.push_str("</svg>");
    svg
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_line(s: &str) -> Vec<Cell> {
        parse(s).into_iter().next().unwrap()
    }

    #[test]
    fn parse_16_color() {
        let line = first_line("\x1b[31mred\x1b[0m plain");
        assert_eq!(line[0].style.fg, Some(Color::Ansi(1)));
        assert_eq!(line[4].style.fg, None);
    }

    #[test]
    fn parse_bold_and_bright() {
        let line = first_line("\x1b[1;32mX\x1b[22mY\x1b[92mZ");
        assert_eq!(line[0].style, CellStyle { fg: Some(Color::Ansi(2)), bg: None, bold: true });
        assert!(!line[1].style.bold);
        assert_eq!(line[2].style.fg, Some(Color::Ansi(10)));
    }

    #[test]
    fn parse_256_color() {
        let line = first_line("\x1b[38;5;196mX\x1b[48;5;232mY");
        assert_eq!(line[0].style.fg, Some(Color::Ansi(196)));
        assert_eq!(line[1].style.bg, Some(Color::Ansi(232)));
        // 196 = 立方 (5,0,0)，232 灰阶 v=8
        assert_eq!(color_rgb(Color::Ansi(196)), [255, 0, 0]);
        assert_eq!(color_rgb(Color::Ansi(232)), [8, 8, 8]);
    }

    #[test]
    fn parse_rgb_color() {
        let line = first_line("\x1b[0;1;38;2;10;20;30mX\x1b[48;2;1;2;3mY\x1b[39;49mZ");
        assert_eq!(line[0].style.fg, Some(Color::Rgb(10, 20, 30)));
        assert!(line[0].style.bold);
        assert_eq!(line[1].style.bg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(line[2].style.fg, None);
        assert_eq!(line[2].style.bg, None);
    }

    #[test]
    fn parse_wide_char() {
        // 宽字符占两列：字符格 + 占位空格
        let line = first_line("中a");
        assert_eq!(line.len(), 3);
        assert_eq!(line[0].ch, '中');
        assert_eq!(line[1].ch, ' ');
        assert_eq!(line[2].ch, 'a');
    }

    #[test]
    fn parse_skip_non_sgr_csi() {
        let line = first_line("\x1b[2K\x1b[1;1Hab");
        assert_eq!(line.iter().map(|c| c.ch).collect::<String>(), "ab");
        assert_eq!(line[0].style.fg, None);
    }

    #[test]
    fn strip_ansi_works() {
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m plain  "), "red plain");
        assert_eq!(strip_ansi("a\n\x1b[32mb\x1b[0m"), "a\nb");
    }

    #[test]
    fn render_smoke() {
        // 渲染依赖系统字体；找不到任何字体时跳过（CI 极简环境）
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        if db.is_empty() {
            eprintln!("no system fonts found, skip render_smoke");
            return;
        }
        let png = render_png("\x1b[1;32m$ cargo test\x1b[0m\n\x1b[38;5;208mwarning\x1b[0m: 中文输出 hello")
            .unwrap();
        assert!(png.len() > 100);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }

    /// 手动跑样例图：cargo test -p hank-server termshot -- --ignored
    #[test]
    #[ignore]
    fn dump_sample() {
        let sample = "\x1b[0;1;38;2;120;180;255m$ cargo test --workspace\x1b[0m\n\
            \x1b[32m   Compiling\x1b[0m hank-server v0.1.0\n\
            \x1b[38;5;208mwarning\x1b[0m\x1b[1m: unused variable\x1b[0m\n\
            \x1b[48;5;236m 中文混排 abc 123 测试 \x1b[0m\n\
            \x1b[1;31merror\x1b[0m: something went wrong";
        let png = render_png(sample).unwrap();
        std::fs::write("/tmp/termshot_sample.png", &png).unwrap();
        eprintln!("wrote /tmp/termshot_sample.png ({} bytes)", png.len());
    }
}
