/**
 * 终端分屏布局树模型（纯函数，不可变更新）。
 * 叶子 = 一个 PTY 会话（term_id），split = 两个子节点的行/列二分。
 */

export type LayoutNode =
  | { kind: "term"; id: string }
  | {
      kind: "split";
      id: string;
      dir: "row" | "col";
      ratio: number;
      a: LayoutNode;
      b: LayoutNode;
    };

export function createLeaf(id: string): LayoutNode {
  return { kind: "term", id };
}

function splitId(): string {
  return `split-${crypto.randomUUID()}`;
}

/** 把 targetId 叶子替换为一个 split 节点（原叶子在 a，新叶子在 b）。未找到返回原树。 */
export function splitNode(
  tree: LayoutNode,
  targetId: string,
  dir: "row" | "col",
  newLeafId: string,
): LayoutNode {
  if (tree.kind === "term") {
    if (tree.id !== targetId) return tree;
    return {
      kind: "split",
      id: splitId(),
      dir,
      ratio: 0.5,
      a: tree,
      b: createLeaf(newLeafId),
    };
  }
  const a = splitNode(tree.a, targetId, dir, newLeafId);
  if (a !== tree.a) return { ...tree, a };
  const b = splitNode(tree.b, targetId, dir, newLeafId);
  if (b !== tree.b) return { ...tree, b };
  return tree;
}

/** 移除 targetId 叶子并收缩其兄弟节点。树被删空时返回 null。 */
export function removeNode(tree: LayoutNode, targetId: string): LayoutNode | null {
  if (tree.kind === "term") return tree.id === targetId ? null : tree;
  const a = removeNode(tree.a, targetId);
  if (a === null) return tree.b;
  if (a !== tree.a) return { ...tree, a };
  const b = removeNode(tree.b, targetId);
  if (b === null) return tree.a;
  if (b !== tree.b) return { ...tree, b };
  return tree;
}

/** 树中第一个（最左/最上）叶子的 term_id。 */
export function firstLeaf(tree: LayoutNode): string | null {
  if (tree.kind === "term") return tree.id;
  return firstLeaf(tree.a);
}

/** 按树的顺序返回所有叶子的 term_id。 */
export function allLeaves(tree: LayoutNode): string[] {
  if (tree.kind === "term") return [tree.id];
  return [...allLeaves(tree.a), ...allLeaves(tree.b)];
}
