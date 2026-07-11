export interface Window {
  start: number;
  end: number;
  offsetY: number;
  totalHeight: number;
}

export function computeWindow(
  scrollTop: number,
  viewportHeight: number,
  rowHeight: number,
  total: number,
  overscan = 20,
): Window {
  const totalHeight = total * rowHeight;
  if (total === 0) return { start: 0, end: 0, offsetY: 0, totalHeight: 0 };
  const first = Math.floor(scrollTop / rowHeight);
  const visible = Math.ceil(viewportHeight / rowHeight);
  const start = Math.max(0, first - overscan);
  const end = Math.min(total, first + visible + overscan);
  return { start, end, offsetY: start * rowHeight, totalHeight };
}

export function rowOffset(rowIndex: number, rowHeight: number): number {
  return rowIndex * rowHeight;
}
