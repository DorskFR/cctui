import type { DiffFile } from "./parse";

export interface TreeFileNode {
  type: "file";
  name: string;
  path: string;
  file: DiffFile;
}

export interface TreeDirNode {
  type: "dir";
  name: string;
  path: string;
  children: TreeNode[];
}

export type TreeNode = TreeDirNode | TreeFileNode;

function ensureDir(parent: TreeDirNode, name: string): TreeDirNode {
  const path = parent.path ? `${parent.path}/${name}` : name;
  for (const child of parent.children) {
    if (child.type === "dir" && child.name === name) return child;
  }
  const dir: TreeDirNode = { type: "dir", name, path, children: [] };
  parent.children.push(dir);
  return dir;
}

function compress(node: TreeDirNode): void {
  for (const child of node.children) {
    if (child.type === "dir") compress(child);
  }
  while (node.children.length === 1 && node.children[0].type === "dir") {
    const only = node.children[0];
    node.name = node.name ? `${node.name}/${only.name}` : only.name;
    node.path = only.path;
    node.children = only.children;
  }
}

export function buildFileTree(files: DiffFile[]): TreeNode[] {
  const root: TreeDirNode = { type: "dir", name: "", path: "", children: [] };
  for (const file of files) {
    const segments = file.filename.split("/");
    const fileName = segments.pop() ?? file.filename;
    let dir = root;
    for (const seg of segments) dir = ensureDir(dir, seg);
    dir.children.push({ type: "file", name: fileName, path: file.filename, file });
  }
  for (const child of root.children) {
    if (child.type === "dir") compress(child);
  }
  return root.children;
}

export function collectFilePaths(node: TreeNode): string[] {
  if (node.type === "file") return [node.path];
  return node.children.flatMap(collectFilePaths);
}

export interface ViewedProgress {
  viewed: number;
  total: number;
}

export function viewedProgress(node: TreeNode, viewed: ReadonlySet<string>): ViewedProgress {
  const paths = collectFilePaths(node);
  return { viewed: paths.filter((p) => viewed.has(p)).length, total: paths.length };
}

export function isFullyViewed(node: TreeNode, viewed: ReadonlySet<string>): boolean {
  const { viewed: v, total } = viewedProgress(node, viewed);
  return total > 0 && v === total;
}
