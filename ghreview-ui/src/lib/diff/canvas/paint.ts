import type { ThemeTokens } from "../../theme/theme";
import type { DiffModel, DiffRow } from "../parse";
import { computeWindow } from "../virtual";
import { CODE_LEFT, GUTTER_WIDTH, MARKER_WIDTH, OVERSCAN } from "./layout";
import { type LineSelection, normalizeSelection } from "./selection";

// The subset of the 2D context API the renderer touches. Both
// CanvasRenderingContext2D (main-thread fallback) and
// OffscreenCanvasRenderingContext2D (worker) satisfy it structurally.
export type Ctx2D = Pick<
  CanvasRenderingContext2D,
  | "fillRect"
  | "clearRect"
  | "fillText"
  | "save"
  | "restore"
  | "setTransform"
  | "beginPath"
  | "rect"
  | "clip"
> & {
  fillStyle: string | CanvasGradient | CanvasPattern;
  font: string;
  textBaseline: CanvasTextBaseline;
  globalAlpha: number;
};

export interface PaintParams {
  model: DiffModel;
  tokens: ThemeTokens;
  scrollTop: number;
  viewportWidth: number;
  viewportHeight: number;
  dpr: number;
  rowHeight: number;
  focusRow: number;
  selection: LineSelection | null;
  fontFamily: string;
  fontSize: number;
}

function rowBg(row: DiffRow, t: ThemeTokens): string {
  switch (row.kind) {
    case "add":
      return t.addBg;
    case "del":
      return t.delBg;
    case "hunk":
      return t.hunkBg;
    case "file":
      return t.bg;
    default:
      return t.bg;
  }
}

function rowFg(row: DiffRow, t: ThemeTokens): string {
  switch (row.kind) {
    case "add":
      return t.addFg;
    case "del":
      return t.delFg;
    case "hunk":
      return t.hunkFg;
    case "file":
      return t.fg;
    default:
      return t.contextFg;
  }
}

function marker(row: DiffRow): string {
  if (row.kind === "add") return "+";
  if (row.kind === "del") return "−";
  return "";
}

export function paint(ctx: Ctx2D, p: PaintParams): void {
  const { model, tokens, scrollTop, viewportWidth, viewportHeight, dpr, rowHeight } = p;

  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.globalAlpha = 1;
  ctx.fillStyle = tokens.bg;
  ctx.fillRect(0, 0, viewportWidth, viewportHeight);
  ctx.font = `${p.fontSize}px ${p.fontFamily}`;
  ctx.textBaseline = "middle";

  const win = computeWindow(scrollTop, viewportHeight, rowHeight, model.rows.length, OVERSCAN);
  const sel = p.selection ? normalizeSelection(p.selection) : null;
  const midOffset = rowHeight / 2;
  const textPad = 6;

  for (let i = win.start; i < win.end; i++) {
    const row = model.rows[i];
    const y = i * rowHeight - scrollTop;

    ctx.fillStyle = rowBg(row, tokens);
    ctx.fillRect(0, y, viewportWidth, rowHeight);

    if (row.kind !== "file" && row.kind !== "hunk") {
      ctx.fillStyle = tokens.gutterBg;
      ctx.fillRect(0, y, GUTTER_WIDTH * 2, rowHeight);
      ctx.fillStyle = tokens.gutterFg;
      if (row.oldLine !== null) {
        ctx.fillText(String(row.oldLine), GUTTER_WIDTH - textPad - 20, y + midOffset);
      }
      if (row.newLine !== null) {
        ctx.fillText(String(row.newLine), GUTTER_WIDTH * 2 - textPad - 20, y + midOffset);
      }
      const m = marker(row);
      if (m) {
        ctx.fillStyle = row.kind === "add" ? tokens.addGlyph : tokens.delGlyph;
        ctx.fillText(m, GUTTER_WIDTH * 2 + MARKER_WIDTH / 2 - 3, y + midOffset);
      }
      ctx.fillStyle = rowFg(row, tokens);
      ctx.fillText(row.content, CODE_LEFT + textPad, y + midOffset);
    } else {
      ctx.fillStyle = rowFg(row, tokens);
      ctx.fillText(row.content, textPad, y + midOffset);
    }

    if (sel && i >= sel.start && i <= sel.end) {
      ctx.fillStyle = tokens.accent;
      ctx.globalAlpha = 0.16;
      ctx.fillRect(0, y, viewportWidth, rowHeight);
      ctx.globalAlpha = 1;
    }
    if (i === p.focusRow) {
      ctx.fillStyle = tokens.accent;
      ctx.fillRect(0, y, 2, rowHeight);
    }
  }

  paintScrollbar(ctx, p);
}

function paintScrollbar(ctx: Ctx2D, p: PaintParams): void {
  const total = p.model.rows.length * p.rowHeight;
  if (total <= p.viewportHeight) return;
  const trackH = p.viewportHeight;
  const thumbH = Math.max(24, (p.viewportHeight / total) * trackH);
  const maxScroll = total - p.viewportHeight;
  const thumbY = (p.scrollTop / maxScroll) * (trackH - thumbH);
  ctx.fillStyle = p.tokens.border;
  ctx.globalAlpha = 0.6;
  ctx.fillRect(p.viewportWidth - 6, thumbY, 4, thumbH);
  ctx.globalAlpha = 1;
}
