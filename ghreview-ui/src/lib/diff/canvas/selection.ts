import type { DiffModel, DiffRow } from "../parse";

export interface LineSelection {
  anchor: number;
  head: number;
}

export interface NormalizedSelection {
  start: number;
  end: number;
}

export interface SelectionEvent {
  start: number;
  end: number;
  fileIndex: number;
  hunkIndex: number;
  rows: DiffRow[];
}

export function normalizeSelection(sel: LineSelection): NormalizedSelection {
  return sel.anchor <= sel.head
    ? { start: sel.anchor, end: sel.head }
    : { start: sel.head, end: sel.anchor };
}

export function selectionRowIndexes(sel: LineSelection): number[] {
  const { start, end } = normalizeSelection(sel);
  const out: number[] = [];
  for (let i = start; i <= end; i++) out.push(i);
  return out;
}

export function selectionEvent(model: DiffModel, sel: LineSelection): SelectionEvent {
  const { start, end } = normalizeSelection(sel);
  const rows = model.rows.slice(start, end + 1);
  const first = model.rows[start];
  return {
    start,
    end,
    fileIndex: first?.fileIndex ?? -1,
    hunkIndex: first?.hunkIndex ?? -1,
    rows,
  };
}

function prefix(row: DiffRow): string {
  if (row.kind === "add") return "+";
  if (row.kind === "del") return "-";
  if (row.kind === "context") return " ";
  return "";
}

export function rangeToClipboardText(model: DiffModel, sel: LineSelection): string {
  const { start, end } = normalizeSelection(sel);
  const lines: string[] = [];
  for (let i = start; i <= end; i++) {
    const row = model.rows[i];
    if (!row) continue;
    lines.push(prefix(row) + row.content);
  }
  return lines.join("\n");
}
