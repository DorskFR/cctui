import type { Component } from "svelte";
import type { NavIndex } from "./navindex";
import type { DiffModel } from "./parse";

export interface DiffRendererProps {
  model: DiffModel;
  nav: NavIndex;
  focusRow: number;
  onFocusRow: (rowIndex: number) => void;
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
