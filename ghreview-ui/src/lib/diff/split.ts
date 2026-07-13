import type { DiffModel, DiffRow } from "./parse";

export interface SplitCell {
  row: DiffRow;
  rowIndex: number;
}

export type SplitRow =
  | { kind: "file"; row: DiffRow; rowIndex: number }
  | { kind: "hunk"; row: DiffRow; rowIndex: number }
  | { kind: "pair"; left: SplitCell | null; right: SplitCell | null };

export interface SplitModel {
  rows: SplitRow[];
  unifiedToSplit: Map<number, number>;
}

export function buildSplitModel(model: DiffModel): SplitModel {
  const rows: SplitRow[] = [];
  const unifiedToSplit = new Map<number, number>();
  const push = (r: SplitRow, ...unifiedIndices: number[]): void => {
    const at = rows.length;
    rows.push(r);
    for (const i of unifiedIndices) unifiedToSplit.set(i, at);
  };

  const src = model.rows;
  let i = 0;
  while (i < src.length) {
    const row = src[i];
    if (row.kind === "file") {
      push({ kind: "file", row, rowIndex: i }, i);
      i += 1;
    } else if (row.kind === "hunk") {
      push({ kind: "hunk", row, rowIndex: i }, i);
      i += 1;
    } else if (row.kind === "context") {
      const cell: SplitCell = { row, rowIndex: i };
      push({ kind: "pair", left: cell, right: cell }, i);
      i += 1;
    } else {
      let j = i;
      const dels: SplitCell[] = [];
      const adds: SplitCell[] = [];
      while (j < src.length && (src[j].kind === "add" || src[j].kind === "del")) {
        if (src[j].kind === "del") dels.push({ row: src[j], rowIndex: j });
        else adds.push({ row: src[j], rowIndex: j });
        j += 1;
      }
      const n = Math.max(dels.length, adds.length);
      for (let k = 0; k < n; k++) {
        const left = dels[k] ?? null;
        const right = adds[k] ?? null;
        const idx: number[] = [];
        if (left) idx.push(left.rowIndex);
        if (right) idx.push(right.rowIndex);
        push({ kind: "pair", left, right }, ...idx);
      }
      i = j;
    }
  }

  return { rows, unifiedToSplit };
}
