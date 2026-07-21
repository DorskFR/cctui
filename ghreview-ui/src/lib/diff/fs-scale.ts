export const BASE_ROW_HEIGHT = 20;
export const BASE_FONT_PX = 12;

const MIN_SCALE = 0.5;
const MAX_SCALE = 3;

export function parseFsScale(raw: string | null | undefined): number {
  const n = Number.parseFloat((raw ?? "").trim());
  if (!Number.isFinite(n) || n <= 0) return 1;
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, n));
}

export function rowHeightFor(scale: number): number {
  return Math.max(1, Math.round(BASE_ROW_HEIGHT * scale));
}

export function fontPxFor(scale: number): number {
  return BASE_FONT_PX * scale;
}

export function readFsScale(el: Element): number {
  return parseFsScale(getComputedStyle(el).getPropertyValue("--fs-scale"));
}
