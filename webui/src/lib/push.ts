import { api } from "./api";
import { path } from "@bindings/routes";
import type { CreateNotificationEndpoint } from "@bindings/CreateNotificationEndpoint";
import type { JsonValue } from "@bindings/serde_json/JsonValue";
import type { NotificationEndpoint } from "@bindings/NotificationEndpoint";
import type { VapidKey } from "@bindings/VapidKey";

export const SW_URL = "/sw.js";
export const WEB_PUSH = "web_push";

export type PushState = "unsupported" | "denied" | "off" | "on";

export interface PushApi {
  vapidKey(): Promise<VapidKey>;
  list(): Promise<NotificationEndpoint[]>;
  create(body: CreateNotificationEndpoint): Promise<NotificationEndpoint>;
  remove(id: string): Promise<void>;
}

export const pushApi: PushApi = {
  vapidKey: () => api.get<VapidKey>(path("get_notifications_vapid_key")),
  list: () => api.get<NotificationEndpoint[]>(path("get_notifications_endpoints")),
  create: (body) => api.post<NotificationEndpoint>(path("post_notifications_endpoints"), body),
  remove: (id) => api.del<void>(path("delete_notifications_endpoints_by_id", { id })),
};

export class PushPermissionError extends Error {
  constructor(readonly permission: NotificationPermission) {
    super(`notification permission ${permission}`);
  }
}

export function pushSupported(): boolean {
  return (
    typeof window !== "undefined" &&
    "serviceWorker" in navigator &&
    "PushManager" in window &&
    "Notification" in window
  );
}

export function decodeKey(b64url: string): Uint8Array<ArrayBuffer> {
  const b64 = b64url.replace(/-/g, "+").replace(/_/g, "/");
  const raw = atob(`${b64}${"=".repeat((4 - (b64.length % 4)) % 4)}`);
  return Uint8Array.from(raw, (c) => c.charCodeAt(0));
}

function sameKey(current: ArrayBuffer | null, wanted: Uint8Array): boolean {
  if (!current || current.byteLength !== wanted.length) return false;
  const bytes = new Uint8Array(current);
  return bytes.every((b, i) => b === wanted[i]);
}

function endpointOf(config: JsonValue): string | undefined {
  if (!config || typeof config !== "object" || Array.isArray(config)) return undefined;
  const endpoint = config.endpoint;
  return typeof endpoint === "string" ? endpoint : undefined;
}

export function findEndpoint(
  rows: NotificationEndpoint[],
  endpoint: string,
): NotificationEndpoint | undefined {
  return rows.find((r) => r.channel === WEB_PUSH && endpointOf(r.config) === endpoint);
}

const BROWSERS: [RegExp, string][] = [
  [/Edg\//, "Edge"],
  [/Firefox\//, "Firefox"],
  [/Chrome\//, "Chrome"],
  [/Safari\//, "Safari"],
];
const SYSTEMS: [RegExp, string][] = [
  [/iPhone|iPad/, "iOS"],
  [/Android/, "Android"],
  [/Mac OS X/, "macOS"],
  [/Windows/, "Windows"],
  [/Linux/, "Linux"],
];

const pick = (ua: string, table: [RegExp, string][]) => table.find(([re]) => re.test(ua))?.[1];

export function browserLabel(ua: string = navigator.userAgent): string {
  const browser = pick(ua, BROWSERS) ?? "Browser";
  const os = pick(ua, SYSTEMS);
  return os ? `${browser} · ${os}` : browser;
}

async function registration(): Promise<ServiceWorkerRegistration> {
  await navigator.serviceWorker.register(SW_URL, { scope: "/" });
  return navigator.serviceWorker.ready;
}

async function browserSubscription(): Promise<PushSubscription | null> {
  const reg = await navigator.serviceWorker.getRegistration();
  return (await reg?.pushManager.getSubscription()) ?? null;
}

export async function pushState(client: PushApi = pushApi): Promise<PushState> {
  if (!pushSupported()) return "unsupported";
  if (Notification.permission === "denied") return "denied";
  const sub = await browserSubscription();
  if (!sub) return "off";
  return findEndpoint(await client.list(), sub.endpoint) ? "on" : "off";
}

export async function subscribe(client: PushApi = pushApi): Promise<NotificationEndpoint> {
  const permission = await Notification.requestPermission();
  if (permission !== "granted") throw new PushPermissionError(permission);
  const reg = await registration();
  const key = decodeKey((await client.vapidKey()).public_key);
  let sub = await reg.pushManager.getSubscription();
  if (sub && !sameKey(sub.options.applicationServerKey, key)) {
    await sub.unsubscribe();
    sub = null;
  }
  sub ??= await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: key });
  const { keys } = sub.toJSON();
  return client.create({
    channel: WEB_PUSH,
    label: browserLabel(),
    config: {
      endpoint: sub.endpoint,
      keys: { p256dh: keys?.p256dh ?? "", auth: keys?.auth ?? "" },
    },
  });
}

export async function unsubscribe(client: PushApi = pushApi): Promise<void> {
  const sub = await browserSubscription();
  if (!sub) return;
  const row = findEndpoint(await client.list(), sub.endpoint);
  if (row) await client.remove(row.id);
  await sub.unsubscribe();
}
