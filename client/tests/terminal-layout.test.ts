// @vitest-environment happy-dom
import { describe, it, expect } from "vitest";
import { mount } from "@vue/test-utils";
import PaneNode from "../src/components/terminal/PaneNode.vue";
import { createLeaf, splitNode, removeNode } from "../src/terminal/layout";

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
