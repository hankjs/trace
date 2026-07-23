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

/** 把 targetId 叶子替换为一个 split 节点（side 决定新叶子放 a 还是 b）。未找到返回原树。 */
export function splitNode(
  tree: LayoutNode,
  targetId: string,
  dir: "row" | "col",
  newLeafId: string,
  side: "a" | "b" = "b",
): LayoutNode {
  if (tree.kind === "term") {
    if (tree.id !== targetId) return tree;
    const newLeaf = createLeaf(newLeafId);
    return {
      kind: "split",
      id: splitId(),
      dir,
      ratio: 0.5,
      a: side === "a" ? newLeaf : tree,
      b: side === "a" ? tree : newLeaf,
    };
  }
  const a = splitNode(tree.a, targetId, dir, newLeafId, side);
  if (a !== tree.a) return { ...tree, a };
  const b = splitNode(tree.b, targetId, dir, newLeafId, side);
  if (b !== tree.b) return { ...tree, b };
  return tree;
}

/** 交换两个叶子的位置（id 互换）。任一 id 不存在时返回原树。 */
export function swapLeaves(tree: LayoutNode, idA: string, idB: string): LayoutNode {
  if (idA === idB) return tree;
  let foundA = false;
  let foundB = false;
  const walk = (node: LayoutNode): LayoutNode => {
    if (node.kind === "term") {
      if (node.id === idA) {
        foundA = true;
        return { ...node, id: idB };
      }
      if (node.id === idB) {
        foundB = true;
        return { ...node, id: idA };
      }
      return node;
    }
    const a = walk(node.a);
    const b = walk(node.b);
    if (a === node.a && b === node.b) return node;
    return { ...node, a, b };
  };
  const result = walk(tree);
  return foundA && foundB ? result : tree;
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

/** 拖拽落点区域：四边 = 朝该方向 split，center = 与目标 pane 交换位置 */
export type DropZone = "left" | "right" | "top" | "bottom" | "center";

/** 拖拽中的实时状态（TerminalView 持有，PaneNode 用于渲染落点高亮） */
export interface PaneDragState {
  dragId: string | null;
  targetId: string | null;
  zone: DropZone | null;
}
