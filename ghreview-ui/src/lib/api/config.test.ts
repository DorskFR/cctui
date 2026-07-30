import { afterEach, describe, expect, it } from "vitest";
import {
  baseUrl,
  basePath,
  configureRuntime,
  getAccount,
  getToken,
  isEmbedded,
  onConfigChange,
  setAccount,
  setToken,
} from "./config";

afterEach(() => {
  configureRuntime(null);
  localStorage.clear();
});

describe("runtime config (embedded)", () => {
  it("is standalone until configured", () => {
    expect(isEmbedded()).toBe(false);
    configureRuntime({ baseUrl: "https://ghreview.example" });
    expect(isEmbedded()).toBe(true);
  });

  it("prefers the injected baseUrl and trims the trailing slash", () => {
    configureRuntime({ baseUrl: "https://ghreview.example/" });
    expect(baseUrl()).toBe("https://ghreview.example");
  });

  it("passes the injected bearer token through", () => {
    setToken("from-localstorage");
    configureRuntime({ token: "session-token" });
    expect(getToken()).toBe("session-token");
  });

  it("treats an injected null token as no token, ignoring localStorage", () => {
    setToken("from-localstorage");
    configureRuntime({ token: null });
    expect(getToken()).toBeNull();
  });

  it("exposes the embed base path without a trailing slash", () => {
    expect(basePath()).toBe("");
    configureRuntime({ basePath: "/review/" });
    expect(basePath()).toBe("/review");
  });

  it("passes the injected account through", () => {
    setAccount("stored");
    configureRuntime({ account: "DorskFR" });
    expect(getAccount()).toBe("DorskFR");
  });
});

describe("config change notification", () => {
  it("notifies subscribers on every reconfiguration until they unsubscribe", () => {
    const seen: (string | null)[] = [];
    const unsubscribe = onConfigChange(() => seen.push(getAccount()));

    configureRuntime({ account: "first" });
    configureRuntime({ account: "second" });
    unsubscribe();
    configureRuntime({ account: "third" });

    expect(seen).toEqual(["first", "second"]);
  });
});

describe("standalone fallback", () => {
  it("reads the token from localStorage when not embedded", () => {
    setToken("stored-token");
    expect(getToken()).toBe("stored-token");
  });
});
