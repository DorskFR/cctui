import { createApp } from "../src/app.ts";

const app = createApp();
const res = await app.request("/v1/openapi.json");
const spec = await res.json();
const out = new URL("../openapi.json", import.meta.url);
await Bun.write(out, `${JSON.stringify(spec, null, 2)}\n`);
console.log(`wrote ${Bun.fileURLToPath(out)}`);
