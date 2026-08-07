/**
 * 终端视觉的唯一来源：调色板 / 底色 / 前景 / 字体栈。
 * 与 handy 对齐：VGA 16 色 + 深色底。
 */

export const TERM_PALETTE = [
  '#000000', '#cd0000', '#00cd00', '#cdcd00',
  '#0000ee', '#cd00cd', '#00cdcd', '#e5e5e5',
  '#7f7f7f', '#ff0000', '#00ff00', '#ffff00',
  '#5c5cff', '#ff00ff', '#00ffff', '#ffffff',
]

export const TERM_BG = '#0d1117'
export const TERM_FG = '#e6edf3'

export const TERM_FONT =
  "Menlo, 'Symbols Nerd Font Mono', 'MesloLGS NF', Monaco, 'Courier New', monospace"

export const TERM_THEME = {
  background: TERM_BG,
  foreground: TERM_FG,
  black: TERM_PALETTE[0],
  red: TERM_PALETTE[1],
  green: TERM_PALETTE[2],
  yellow: TERM_PALETTE[3],
  blue: TERM_PALETTE[4],
  magenta: TERM_PALETTE[5],
  cyan: TERM_PALETTE[6],
  white: TERM_PALETTE[7],
  brightBlack: TERM_PALETTE[8],
  brightRed: TERM_PALETTE[9],
  brightGreen: TERM_PALETTE[10],
  brightYellow: TERM_PALETTE[11],
  brightBlue: TERM_PALETTE[12],
  brightMagenta: TERM_PALETTE[13],
  brightCyan: TERM_PALETTE[14],
  brightWhite: TERM_PALETTE[15],
}
