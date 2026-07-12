import type { DiffRenderer } from "../diff/renderer";
import CanvasDiffView from "./CanvasDiffView.svelte";

export const CanvasDiffRenderer: DiffRenderer = {
  kind: "canvas",
  component: CanvasDiffView,
};
