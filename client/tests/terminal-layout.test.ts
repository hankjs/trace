// @vitest-environment happy-dom
import { describe, it, expect } from "vitest";
import { mount } from "@vue/test-utils";
import PaneNode from "../src/components/terminal/PaneNode.vue";
import { createLeaf, splitNode, removeNode, swapLeaves, allLeaves } from "../src/terminal/layout";

function mountWithProvides(node: any) {
  return mount(PaneNode, {
    props: { node, activePaneId: "" },
    global: {
      provide: {
        registerTermEl: () => {},
        paneInfos: {},
        paneTitles: {},
        paneRename: { id: null },
      },
    },
  });
}

describe("PaneNode 树收缩渲染", () => {
  it("split -> 关闭右 pane 后，DOM 只剩一个 .pane", async () => {
    let tree: any = createLeaf("A");
    tree = splitNode(tree, "A", "row", "B");
    const wrapper = mountWithProvides(tree);
    expect(wrapper.findAll(".pane").length).toBe(2);

    const newRoot = removeNode(tree, "B");
    expect(newRoot).not.toBeNull();
    await wrapper.setProps({ node: newRoot });
    expect(wrapper.findAll(".pane").length).toBe(1);
  });

  it("split -> 关闭左 pane 后，DOM 只剩一个 .pane", async () => {
    let tree: any = createLeaf("A");
    tree = splitNode(tree, "A", "row", "B");
    const wrapper = mountWithProvides(tree);
    expect(wrapper.findAll(".pane").length).toBe(2);

    const newRoot = removeNode(tree, "A");
    await wrapper.setProps({ node: newRoot });
    expect(wrapper.findAll(".pane").length).toBe(1);
  });

  it("removeNode 删除后树里不再含该 id", () => {
    let tree: any = createLeaf("A");
    tree = splitNode(tree, "A", "row", "B");
    const newRoot = removeNode(tree, "B");
    expect(JSON.stringify(newRoot)).not.toContain('"B"');
  });
});

describe("拖拽移动 pane 的树操作", () => {
  it("splitNode side=a：新叶子插到目标左侧/上侧", () => {
    let tree: any = createLeaf("A");
    tree = splitNode(tree, "A", "row", "B", "a");
    expect(tree.kind).toBe("split");
    expect(tree.a).toEqual({ kind: "term", id: "B" });
    expect(tree.b).toEqual({ kind: "term", id: "A" });
  });

  it("splitNode 默认 side=b：新叶子在目标右侧/下侧（向后兼容）", () => {
    let tree: any = createLeaf("A");
    tree = splitNode(tree, "A", "col", "B");
    expect(tree.dir).toBe("col");
    expect(tree.a.id).toBe("A");
    expect(tree.b.id).toBe("B");
  });

  it("removeNode + splitNode 组合实现拖走再插入", () => {
    let tree: any = createLeaf("A");
    tree = splitNode(tree, "A", "row", "B");
    tree = splitNode(tree, "B", "col", "C");
    // 拖 A 到 C 的上边缘：removeNode 摘除 A，再 splitNode 插到 C 的 a 侧
    const removed = removeNode(tree, "A");
    expect(removed).not.toBeNull();
    const newRoot: any = splitNode(removed!, "C", "col", "A", "a");
    expect(allLeaves(newRoot)).toEqual(["B", "A", "C"]);
  });

  it("swapLeaves 交换两个叶子的位置", () => {
    let tree: any = createLeaf("A");
    tree = splitNode(tree, "A", "row", "B");
    tree = splitNode(tree, "B", "col", "C");
    const swapped = swapLeaves(tree, "A", "C");
    expect(allLeaves(swapped)).toEqual(["C", "B", "A"]);
    // 原树不可变
    expect(allLeaves(tree)).toEqual(["A", "B", "C"]);
  });

  it("swapLeaves 处理嵌套与非法 id", () => {
    let tree: any = createLeaf("A");
    tree = splitNode(tree, "A", "row", "B");
    expect(swapLeaves(tree, "A", "X")).toBe(tree); // 目标不存在：原样返回
    expect(swapLeaves(tree, "A", "A")).toBe(tree); // 自身：原样返回
  });
});
