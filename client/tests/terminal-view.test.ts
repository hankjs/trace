// @vitest-environment happy-dom
/**
 * TerminalView 的分屏/关闭全流程测试（mock tauri + xterm）。
 * 复现用户报告：⌘W 关闭 pane 后 DOM 里残留空框。
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";

// ---- mock xterm ----
const h = vi.hoisted(() => {
  const keyHandlers = new Map<string, (e: any) => boolean>();
  class FakeTerminal {
    static byEl = new WeakMap<HTMLElement, FakeTerminal>();
    static seq = 0;
    id: string;
    element: HTMLElement | undefined;
    cols = 80;
    rows = 24;
    disposed = false;
    constructor(_opts: any) {
      this.id = `term-${++FakeTerminal.seq}`;
    }
    loadAddon() {}
    open(el: HTMLElement) {
      const dom = el.ownerDocument.createElement("div");
      dom.className = "xterm";
      el.appendChild(dom);
      this.element = dom;
      FakeTerminal.byEl.set(el, this);
    }
    write() {}
    focus() {}
    blur() {}
    onData() {}
    onBell() {}
    parser = { registerOscHandler: () => {} };
    attachCustomKeyEventHandler(cb: (e: any) => boolean) {
      keyHandlers.set(this.id, cb);
    }
    dispose() {
      this.disposed = true;
      this.element?.parentElement?.removeChild(this.element);
    }
  }
  return { keyHandlers, FakeTerminal };
});
const { keyHandlers, FakeTerminal } = h;

vi.mock("@xterm/xterm", () => ({ Terminal: h.FakeTerminal }));
vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    fit() {}
    proposeDimensions() {
      return { cols: 80, rows: 24 };
    }
  },
}));
vi.mock("@xterm/addon-search", () => ({ SearchAddon: class {} }));
vi.mock("@xterm/addon-canvas", () => ({ CanvasAddon: class {} }));
vi.mock("@xterm/xterm/css/xterm.css", () => ({}));

// ---- mock tauri ----
let ptySeq = 0;
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string, args: any) => {
    if (cmd === "term_create") {
      const id = `pty-${++ptySeq}`;
      return { id, shell: "/bin/zsh", cwd: "/Users/admin", foreground_cmd: "zsh", alive: true, created_at: "" };
    }
    if (cmd === "term_list") return [];
    if (cmd === "term_foreground_cwd") return "/Users/admin";
    return null;
  }),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onFocusChanged: vi.fn(async () => () => {}),
    onDragDropEvent: vi.fn(async () => () => {}),
  }),
}));

import TerminalView from "../src/views/TerminalView.vue";
import { termTabs, activeTermTabId } from "../src/terminal/termTabs";

describe("TerminalView 分屏/关闭流程", () => {
  beforeEach(() => {
    keyHandlers.clear();
    // tab 状态是模块级的，用例间需手动清理
    termTabs.value = [];
    activeTermTabId.value = "";
  });

  it("新建 -> 分屏 -> ⌘W 关闭后 DOM 不残留 pane", async () => {
    const wrapper = mount(TerminalView, { attachTo: document.body });
    await flushPromises();

    // 初始：1 个 tab 1 个 pane
    expect(wrapper.findAll(".pane").length).toBe(1);

    // 模拟在焦点 pane 上按 ⌘D 向右分屏
    const handlers = [...keyHandlers.values()];
    expect(handlers.length).toBeGreaterThan(0);
    handlers[handlers.length - 1]({ type: "keydown", metaKey: true, key: "d", shiftKey: false, ctrlKey: false, altKey: false });
    await flushPromises();
    expect(wrapper.findAll(".pane").length).toBe(2);

    // 模拟在焦点 pane 上按 ⌘W：找到新 pane 的 key handler
    // keyHandlers 按 FakeTerminal 实例 id 存，取最后一个（新 attach 的）
    const last = [...keyHandlers.values()].at(-1)!;
    last({ type: "keydown", metaKey: true, key: "w", ctrlKey: false, altKey: false });
    await flushPromises();

    expect(wrapper.findAll(".pane").length).toBe(1);
    expect(wrapper.findAll(".pane-titlebar").length).toBe(1);

    wrapper.unmount();
  });
});
