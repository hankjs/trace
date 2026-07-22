// @vitest-environment happy-dom
/** serializeScreen 端到端：颜色 SGR 保留 + 宽字符不多空格 */
import { describe, it, expect } from "vitest";
import { Terminal } from "@xterm/xterm";
import { registerTerm, unregisterTerm, serializeScreen } from "../src/terminal/screenRegistry";

function openTerm(cols = 40): Terminal {
  const el = document.createElement("div");
  document.body.appendChild(el);
  const term = new Terminal({ cols, rows: 5 });
  term.open(el);
  return term;
}

async function writeAndWait(term: Terminal, data: string): Promise<void> {
  await new Promise<void>((resolve) => term.write(data, resolve));
}

describe("serializeScreen", () => {
  it("保留 256 色与 RGB 前景色", async () => {
    const term = openTerm();
    registerTerm("t1", term);
    await writeAndWait(term, "\x1b[38;5;169mA\x1b[0m\x1b[38;2;255;100;50mB\x1b[0m");
    const out = serializeScreen("t1")!;
    expect(out).toContain("38;5;169");
    expect(out).toContain("38;2;255;100;50");
    unregisterTerm("t1");
    term.dispose();
  });

  it("中文宽字符不产生多余空格", async () => {
    const term = openTerm();
    registerTerm("t2", term);
    await writeAndWait(term, "中文ab");
    const out = serializeScreen("t2")!;
    // 去掉 SGR 后应为 "中文ab"，而不是 "中 文 ab"
    const plain = out.replace(/\x1b\[[0-9;]*m/g, "").replace(/\r\n/g, "\n").split("\n")[0];
    expect(plain.startsWith("中文ab")).toBe(true);
    unregisterTerm("t2");
    term.dispose();
  });

  it("未注册的终端返回 null", () => {
    expect(serializeScreen("nonexistent")).toBeNull();
  });
});
