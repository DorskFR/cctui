// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CreateNotificationEndpoint } from "@bindings/CreateNotificationEndpoint";
import type { NotificationEndpoint } from "@bindings/NotificationEndpoint";
import type { VapidKey } from "@bindings/VapidKey";
import {
  type PushApi,
  PushPermissionError,
  SW_URL,
  browserLabel,
  decodeKey,
  pushState,
  subscribe,
  unsubscribe,
} from "./push";

const VAPID =
  "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8";
const ENDPOINT = "https://push.example.net/push/abc";

class FakeSubscription {
  unsubscribe = vi.fn(async () => true);
  constructor(
    readonly endpoint: string,
    readonly options: { applicationServerKey: ArrayBufferLike | null },
  ) {}
  toJSON() {
    return { endpoint: this.endpoint, keys: { p256dh: "pub", auth: "sec" } };
  }
}

let current: FakeSubscription | null;
const pushManager = {
  getSubscription: vi.fn(async () => current),
  subscribe: vi.fn(async (opts: { applicationServerKey: Uint8Array }) => {
    current = new FakeSubscription(ENDPOINT, {
      applicationServerKey: opts.applicationServerKey.slice().buffer,
    });
    return current;
  }),
};
const registration = { pushManager };
const serviceWorker = {
  register: vi.fn(async () => registration),
  ready: Promise.resolve(registration),
  getRegistration: vi.fn(async () => registration),
};
let permission: NotificationPermission;
const requestPermission = vi.fn(async () => permission);

class FakeNotification {
  static requestPermission = requestPermission;
  static get permission() {
    return permission;
  }
}

function row(id: string, endpoint: string, channel = "web_push"): NotificationEndpoint {
  return {
    id,
    channel,
    label: null,
    config: { endpoint },
    enabled: true,
    created_at: "2026-11-16T00:00:00Z",
    last_ok_at: null,
  };
}

function client(rows: NotificationEndpoint[] = []) {
  const api = {
    vapidKey: vi.fn(async (): Promise<VapidKey> => ({ public_key: VAPID })),
    list: vi.fn(async () => rows),
    create: vi.fn(async (b: CreateNotificationEndpoint) => ({
      ...row("new", ENDPOINT),
      label: b.label,
    })),
    remove: vi.fn(async (_id: string) => undefined),
  };
  return api satisfies PushApi;
}

beforeEach(() => {
  current = null;
  permission = "default";
  vi.clearAllMocks();
  Object.defineProperty(navigator, "serviceWorker", { value: serviceWorker, configurable: true });
  vi.stubGlobal("PushManager", class {});
  vi.stubGlobal("Notification", FakeNotification);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("push subscription", () => {
  it("subscribe registers the worker, subscribes with the VAPID key, posts the subscription", async () => {
    permission = "granted";
    const api = client();
    await subscribe(api);

    expect(serviceWorker.register).toHaveBeenCalledWith(SW_URL, { scope: "/" });
    const opts = pushManager.subscribe.mock.calls[0][0] as {
      userVisibleOnly: boolean;
      applicationServerKey: Uint8Array;
    };
    expect(opts.userVisibleOnly).toBe(true);
    expect(Array.from(opts.applicationServerKey)).toEqual(Array.from(decodeKey(VAPID)));
    expect(api.create).toHaveBeenCalledWith({
      channel: "web_push",
      label: expect.any(String),
      config: { endpoint: ENDPOINT, keys: { p256dh: "pub", auth: "sec" } },
    });
  });

  it("subscribe refuses without notification permission and posts nothing", async () => {
    permission = "denied";
    const api = client();
    await expect(subscribe(api)).rejects.toBeInstanceOf(PushPermissionError);
    expect(pushManager.subscribe).not.toHaveBeenCalled();
    expect(api.create).not.toHaveBeenCalled();
  });

  it("subscribe replaces a browser subscription made with another server key", async () => {
    permission = "granted";
    const stale = new FakeSubscription("https://push.example.net/old", {
      applicationServerKey: new Uint8Array([1, 2, 3]).buffer,
    });
    current = stale;
    await subscribe(client());
    expect(stale.unsubscribe).toHaveBeenCalled();
    expect(pushManager.subscribe).toHaveBeenCalledTimes(1);
  });

  it("subscribe reuses a subscription already made with the server key", async () => {
    permission = "granted";
    const kept = new FakeSubscription(ENDPOINT, {
      applicationServerKey: decodeKey(VAPID).slice().buffer,
    });
    current = kept;
    const api = client();
    await subscribe(api);
    expect(kept.unsubscribe).not.toHaveBeenCalled();
    expect(pushManager.subscribe).not.toHaveBeenCalled();
    expect(api.create).toHaveBeenCalledTimes(1);
  });

  it("unsubscribe deletes this browser's endpoint, then drops the browser subscription", async () => {
    const sub = new FakeSubscription(ENDPOINT, { applicationServerKey: null });
    current = sub;
    const api = client([
      row("ntfy-1", ENDPOINT, "ntfy"),
      row("other", "https://push.example.net/other"),
      row("mine", ENDPOINT),
    ]);
    await unsubscribe(api);
    expect(api.remove).toHaveBeenCalledWith("mine");
    expect(api.remove).toHaveBeenCalledTimes(1);
    expect(sub.unsubscribe).toHaveBeenCalled();
  });

  it("unsubscribe without a browser subscription calls nothing", async () => {
    const api = client([row("mine", ENDPOINT)]);
    await unsubscribe(api);
    expect(api.list).not.toHaveBeenCalled();
    expect(api.remove).not.toHaveBeenCalled();
  });

  it("is on only when the server still lists this browser's subscription", async () => {
    permission = "granted";
    expect(await pushState(client([row("mine", ENDPOINT)]))).toBe("off");
    current = new FakeSubscription(ENDPOINT, { applicationServerKey: null });
    expect(await pushState(client([row("mine", ENDPOINT)]))).toBe("on");
    expect(await pushState(client([row("other", "https://push.example.net/other")]))).toBe("off");
    permission = "denied";
    expect(await pushState(client())).toBe("denied");
  });

  it("is unsupported without the Push API", async () => {
    Reflect.deleteProperty(globalThis, "PushManager");
    Reflect.deleteProperty(window, "PushManager");
    expect(await pushState(client())).toBe("unsupported");
  });
});

describe("push helpers", () => {
  it("decodes a base64url key to its 65 point bytes", () => {
    const key = decodeKey(VAPID);
    expect(key.length).toBe(65);
    expect(key[0]).toBe(4);
  });

  it("labels the device by browser and system", () => {
    const firefox = "Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0";
    expect(browserLabel(firefox)).toBe("Firefox · Linux");
    const iphone =
      "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 " +
      "(KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";
    expect(browserLabel(iphone)).toBe("Safari · iOS");
    expect(browserLabel("curl/8")).toBe("Browser");
  });
});
