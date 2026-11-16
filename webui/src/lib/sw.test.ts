import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";

type Event = Record<string, unknown>;
type Handler = (event: Event) => void;

const CHROME =
  "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0 Safari/537.36";
const IPHONE =
  "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 " +
  "(KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";
const SOURCE = readFileSync(join(process.cwd(), "static/sw.js"), "utf8");

function load(ua: string) {
  const listeners = new Map<string, Handler>();
  const shown = [
    { tag: "t-1", close: vi.fn() },
    { tag: "t-2", close: vi.fn() },
  ];
  const appWindow = { url: "https://cctui.example/sessions/s1", focus: vi.fn(async () => null) };
  const self = {
    navigator: { userAgent: ua },
    location: { origin: "https://cctui.example" },
    registration: {
      showNotification: vi.fn(async (_title: string, _opts: Record<string, unknown>) => undefined),
      getNotifications: vi.fn(async ({ tag }: { tag: string }) =>
        shown.filter((n) => n.tag === tag),
      ),
    },
    clients: {
      matchAll: vi.fn(async () => [appWindow]),
      openWindow: vi.fn(async (_url: string) => null),
      claim: vi.fn(async () => undefined),
    },
    skipWaiting: vi.fn(),
    addEventListener: (type: string, fn: Handler) => listeners.set(type, fn),
  };
  new Function("self", SOURCE)(self);

  async function fire(type: string, event: Event) {
    let pending: unknown = null;
    const waitUntil = (p: unknown) => {
      pending = p;
    };
    listeners.get(type)?.({ ...event, waitUntil });
    await pending;
  }
  const push = (payload: unknown) => fire("push", { data: { json: () => payload } });
  return { self, shown, appWindow, fire, push };
}

describe("service worker", () => {
  it("shows a pushed notice with its tag, link and confirm-page actions", async () => {
    const sw = load(CHROME);
    await sw.push({
      type: "show",
      title: "Approve deploy",
      body: "prod",
      tag: "t-9",
      renotify: true,
      data: { url: "https://cctui.example/sessions/s1" },
      actions: [{ action: "a0", title: "Approve", url: "https://cctui.example/n/tokA" }],
    });
    expect(sw.self.registration.showNotification).toHaveBeenCalledWith("Approve deploy", {
      body: "prod",
      tag: "t-9",
      renotify: true,
      data: {
        url: "https://cctui.example/sessions/s1",
        actions: { a0: "https://cctui.example/n/tokA" },
      },
      actions: [{ action: "a0", title: "Approve" }],
    });
  });

  it("a close push closes only the notices with that tag", async () => {
    const sw = load(CHROME);
    await sw.push({ type: "close", tag: "t-1", title: "Resolved" });
    expect(sw.shown[0].close).toHaveBeenCalled();
    expect(sw.shown[1].close).not.toHaveBeenCalled();
    expect(sw.self.registration.showNotification).not.toHaveBeenCalled();
  });

  it("on WebKit a close push leaves a quiet resolved line in its place", async () => {
    const sw = load(IPHONE);
    await sw.push({ type: "close", tag: "t-1", title: "Resolved" });
    expect(sw.shown[0].close).toHaveBeenCalled();
    expect(sw.self.registration.showNotification).toHaveBeenCalledWith("Resolved", {
      tag: "t-1",
      silent: true,
    });
  });

  it("an action click opens its confirm page and never posts", async () => {
    const sw = load(CHROME);
    const notification = {
      close: vi.fn(),
      data: {
        url: "https://cctui.example/sessions/s1",
        actions: { a0: "https://cctui.example/n/tokA" },
      },
    };
    await sw.fire("notificationclick", { action: "a0", notification });
    expect(notification.close).toHaveBeenCalled();
    expect(sw.self.clients.openWindow).toHaveBeenCalledWith("https://cctui.example/n/tokA");
  });

  it("a body click focuses the window already on the link", async () => {
    const sw = load(CHROME);
    const notification = { close: vi.fn(), data: { url: "https://cctui.example/sessions/s1" } };
    await sw.fire("notificationclick", { action: "", notification });
    expect(sw.appWindow.focus).toHaveBeenCalled();
    expect(sw.self.clients.openWindow).not.toHaveBeenCalled();
  });

  it("a non-http link falls back to the app root", async () => {
    const sw = load(CHROME);
    const notification = { close: vi.fn(), data: { url: "ftp://files.example/x" } };
    await sw.fire("notificationclick", { action: "", notification });
    expect(sw.self.clients.openWindow).toHaveBeenCalledWith("https://cctui.example/");
  });

  it("ignores a push it cannot parse", async () => {
    const sw = load(CHROME);
    const json = () => {
      throw new SyntaxError("bad");
    };
    await sw.fire("push", { data: { json } });
    expect(sw.self.registration.showNotification).not.toHaveBeenCalled();
  });
});
