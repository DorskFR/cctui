import { mount } from "svelte";
import App from "./App.svelte";
import "./app.css";
import { CanvasDiffRenderer } from "./lib/components/CanvasDiffRenderer";
import { DomDiffRenderer } from "./lib/components/DomDiffRenderer";
import { registerRenderer } from "./lib/diff/renderer";
import { initTheme } from "./lib/theme/theme";

initTheme();
registerRenderer(DomDiffRenderer);
registerRenderer(CanvasDiffRenderer);

const app = mount(App, { target: document.getElementById("app") as HTMLElement });

export default app;
