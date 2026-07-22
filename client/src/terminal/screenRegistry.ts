/**
 * 共享的 xterm 实例注册表 + 当前屏幕序列化。
 * 供远程工具（admin/微信查看终端）获取与桌面端一致的屏幕快照,
 * 而不是回放包含中间帧的 PTY 原始流。
 */
import type { Terminal, IBufferCell, IBufferLine } from "@xterm/xterm";

const terms = new Map<string, Terminal>();

export function registerTerm(id: string, term: Terminal) {
  terms.set(id, term);
}

export function unregisterTerm(id: string) {
  terms.delete(id);
}

// xterm v6 的 getFgColorMode/getBgColorMode 是位掩码（mode << 24）
function colorMode(raw: number): number {
  return raw >> 24;
}
const CM_P16 = 1;
const CM_P256 = 2;
const CM_RGB = 3;

function fgSgr(cell: IBufferCell): string | null {
  const mode = colorMode(cell.getFgColorMode());
  const c = cell.getFgColor();
  if (mode === CM_RGB) return `38;2;${(c >> 16) & 255};${(c >> 8) & 255};${c & 255}`;
  if (mode === CM_P256) return `38;5;${c}`;
  if (mode === CM_P16) return String(c < 8 ? 30 + c : 90 + c - 8);
  return null;
}

function bgSgr(cell: IBufferCell): string | null {
  const mode = colorMode(cell.getBgColorMode());
  const c = cell.getBgColor();
  if (mode === CM_RGB) return `48;2;${(c >> 16) & 255};${(c >> 8) & 255};${c & 255}`;
  if (mode === CM_P256) return `48;5;${c}`;
  if (mode === CM_P16) return String(c < 8 ? 40 + c : 100 + c - 8);
  return null;
}

/** 单元格的完整 SGR（以 reset 开头，幂等，便于 run 之间直接切换） */
function attrsOf(cell: IBufferCell): string {
  const parts: string[] = ["0"];
  if (cell.isBold()) parts.push("1");
  if (cell.isDim()) parts.push("2");
  if (cell.isItalic()) parts.push("3");
  if (cell.isUnderline()) parts.push("4");
  if (cell.isBlink()) parts.push("5");
  if (cell.isInverse()) parts.push("7");
  if (cell.isInvisible()) parts.push("8");
  if (cell.isStrikethrough()) parts.push("9");
  const fg = fgSgr(cell);
  if (fg) parts.push(fg);
  const bg = bgSgr(cell);
  if (bg) parts.push(bg);
  return `\x1b[${parts.join(";")}m`;
}

function serializeLine(line: IBufferLine, cols: number): string {
  let result = "";
  let cur = "";
  const width = Math.min(cols, line.length);
  for (let x = 0; x < width; x++) {
    const cell = line.getCell(x);
    if (!cell) break;
    // 宽字符（CJK/emoji）的第二格 width=0，跳过，否则会多出空格
    if (cell.getWidth() === 0) continue;
    const a = attrsOf(cell);
    if (a !== cur) {
      result += a;
      cur = a;
    }
    result += cell.getChars() || " ";
  }
  return result + "\x1b[0m";
}

/**
 * 序列化某终端的当前屏幕（含滚动区最近 maxLines 行）为带 SGR 的文本。
 * 终端未在本视图附着（从未打开终端页）时返回 null，调用方需降级处理。
 */
export function serializeScreen(id: string, maxLines = 200): string | null {
  const term = terms.get(id);
  if (!term) return null;
  const buf = term.buffer.active;
  const start = Math.max(0, buf.length - maxLines);
  const out: string[] = [];
  for (let y = start; y < buf.length; y++) {
    const line = buf.getLine(y);
    out.push(line ? serializeLine(line, term.cols) : "");
  }
  return out.join("\r\n");
}
