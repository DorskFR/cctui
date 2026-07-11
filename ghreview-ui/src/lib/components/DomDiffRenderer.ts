import type { DiffRenderer } from "../diff/renderer";
import DiffView from "./DiffView.svelte";

export const DomDiffRenderer: DiffRenderer = {
  kind: "dom",
  component: DiffView,
};
