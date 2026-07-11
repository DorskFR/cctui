import { mount } from "svelte";
import App from "./App.svelte";
import "./app.css";
import { DomDiffRenderer } from "./lib/components/DomDiffRenderer";
import { registerRenderer } from "./lib/diff/renderer";

registerRenderer(DomDiffRenderer);

const app = mount(App, { target: document.getElementById("app") as HTMLElement });

export default app;
