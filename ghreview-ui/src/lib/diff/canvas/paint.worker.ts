import type { ThemeTokens } from "../../theme/theme";
import type { DiffModel } from "../parse";
import { type Ctx2D, type PaintParams, paint } from "./paint";
import type { LineSelection } from "./selection";

type InitMsg = { type: "init"; canvas: OffscreenCanvas };
type ModelMsg = { type: "model"; model: DiffModel };
type FrameMsg = {
  type: "frame";
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
};
export type WorkerMsg = InitMsg | ModelMsg | FrameMsg;

let canvas: OffscreenCanvas | null = null;
let ctx: Ctx2D | null = null;
let model: DiffModel | null = null;

self.onmessage = (e: MessageEvent<WorkerMsg>) => {
  const msg = e.data;
  if (msg.type === "init") {
    canvas = msg.canvas;
    ctx = canvas.getContext("2d") as unknown as Ctx2D | null;
    return;
  }
  if (msg.type === "model") {
    model = msg.model;
    return;
  }
  if (!canvas || !ctx || !model) return;
  const w = Math.round(msg.viewportWidth * msg.dpr);
  const h = Math.round(msg.viewportHeight * msg.dpr);
  if (canvas.width !== w) canvas.width = w;
  if (canvas.height !== h) canvas.height = h;
  const params: PaintParams = {
    model,
    tokens: msg.tokens,
    scrollTop: msg.scrollTop,
    viewportWidth: msg.viewportWidth,
    viewportHeight: msg.viewportHeight,
    dpr: msg.dpr,
    rowHeight: msg.rowHeight,
    focusRow: msg.focusRow,
    selection: msg.selection,
    fontFamily: msg.fontFamily,
    fontSize: msg.fontSize,
  };
  paint(ctx, params);
};
