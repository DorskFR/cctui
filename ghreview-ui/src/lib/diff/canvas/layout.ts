import type { DiffModel, DiffRow } from "../parse";

// Shared with the DOM baseline (DiffView.svelte imports this) so a row's document
// offset is identical whichever renderer paints it — nav index and scroll stay swappable.
export const ROW_HEIGHT = 20;
export const OVERSCAN = 20;

export const GUTTER_WIDTH = 48;
export const MARKER_WIDTH = 14;
export const CODE_LEFT = GUTTER_WIDTH * 2 + MARKER_WIDTH;

export type HitRegion = "oldGutter" | "newGutter" | "marker" | "code";

export interface Hit {
  rowIndex: number;
  region: HitRegion;
  row: DiffRow;
  fileIndex: number;
  hunkIndex: number;
}

export function totalHeight(model: DiffModel, rowHeight = ROW_HEIGHT): number {
  return model.rows.length * rowHeight;
}

export function rowTop(rowIndex: number, rowHeight = ROW_HEIGHT): number {
  return rowIndex * rowHeight;
}

export function rowAtY(y: number, rowCount: number, rowHeight = ROW_HEIGHT): number {
  if (y < 0) return -1;
  const idx = Math.floor(y / rowHeight);
  return idx >= rowCount ? -1 : idx;
}

export function regionAtX(x: number): HitRegion {
  if (x < GUTTER_WIDTH) return "oldGutter";
  if (x < GUTTER_WIDTH * 2) return "newGutter";
  if (x < CODE_LEFT) return "marker";
  return "code";
}

export function hitTest(
  model: DiffModel,
  x: number,
  yLocal: number,
  scrollTop: number,
  rowHeight = ROW_HEIGHT,
): Hit | null {
  const rowIndex = rowAtY(yLocal + scrollTop, model.rows.length, rowHeight);
  if (rowIndex < 0) return null;
  const row = model.rows[rowIndex];
  return {
    rowIndex,
    region: regionAtX(x),
    row,
    fileIndex: row.fileIndex,
    hunkIndex: row.hunkIndex,
  };
}

export function anchorScreenY(rowIndex: number, scrollTop: number, rowHeight = ROW_HEIGHT): number {
  return rowIndex * rowHeight - scrollTop;
}

export function maxScroll(
  model: DiffModel,
  viewportHeight: number,
  rowHeight = ROW_HEIGHT,
): number {
  return Math.max(0, totalHeight(model, rowHeight) - viewportHeight);
}

export function clampScroll(
  scrollTop: number,
  model: DiffModel,
  viewportHeight: number,
  rowHeight = ROW_HEIGHT,
): number {
  const max = maxScroll(model, viewportHeight, rowHeight);
  if (scrollTop < 0) return 0;
  return scrollTop > max ? max : scrollTop;
}

export function scrollToRow(
  rowIndex: number,
  scrollTop: number,
  viewportHeight: number,
  model: DiffModel,
  rowHeight = ROW_HEIGHT,
): number {
  const top = rowTop(rowIndex, rowHeight);
  const bottom = top + rowHeight;
  if (top < scrollTop || bottom > scrollTop + viewportHeight) {
    return clampScroll(top - viewportHeight / 3, model, viewportHeight, rowHeight);
  }
  return scrollTop;
}
