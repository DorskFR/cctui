import type { Component } from "svelte";
import type { ReviewController } from "../review/anchors";
import type { SelectionEvent } from "./canvas/selection";
import type { NavIndex } from "./navindex";
import type { DiffModel } from "./parse";

export interface DiffRendererProps {
  model: DiffModel;
  nav: NavIndex;
  focusRow: number;
  onFocusRow: (rowIndex: number) => void;
  onSelectRange?: (event: SelectionEvent) => void;
  review?: ReviewController;
  mode?: "unified" | "split";
  owner?: string;
  repo?: string;
  account?: string;
}

export interface DiffRenderer {
  readonly kind: "dom" | "canvas";
  readonly component: Component<DiffRendererProps>;
}

const registry = new Map<string, DiffRenderer>();

export function registerRenderer(renderer: DiffRenderer): void {
  registry.set(renderer.kind, renderer);
}

export function getRenderer(kind: "dom" | "canvas" = "dom"): DiffRenderer | undefined {
  return registry.get(kind) ?? registry.get("dom");
}

export type RendererKind = "dom" | "canvas";

const RENDERER_STORAGE_KEY = "ghreview:renderer";

export function getPreferredRendererKind(): RendererKind {
  const v = localStorage.getItem(RENDERER_STORAGE_KEY);
  return v === "canvas" ? "canvas" : "dom";
}

export function setPreferredRendererKind(kind: RendererKind): void {
  localStorage.setItem(RENDERER_STORAGE_KEY, kind);
}
