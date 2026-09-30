import { describe, expect, it } from "vitest";
import { backendPath, HOST_CONTEXT_KEY, PLUGIN_ID } from "./host";

describe("plugin identity", () => {
  it("pins the manifest id and the host context key the server agrees on", () => {
    expect(PLUGIN_ID).toBe("ghreview");
    expect(HOST_CONTEXT_KEY).toBe("cctui:host");
  });
});

describe("backendPath", () => {
  it("routes a backend path through the plugin proxy", () => {
    expect(backendPath("/v1/events")).toBe("/api/v1/plugins/ghreview/backend/v1/events");
  });

  it("tolerates a path given without a leading slash", () => {
    expect(backendPath("v1/status")).toBe("/api/v1/plugins/ghreview/backend/v1/status");
  });
});
