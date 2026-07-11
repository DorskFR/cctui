import { createApp } from "./app.ts";

const app = createApp();
const port = Number(process.env.PORT ?? 8790);

export default {
  port,
  fetch: app.fetch,
};
