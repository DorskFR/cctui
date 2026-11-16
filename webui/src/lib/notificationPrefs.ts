import type { NotificationChannelPref } from "@bindings/NotificationChannelPref";
import type { NotificationSettings } from "@bindings/NotificationSettings";
import type { NotificationStage } from "@bindings/NotificationStage";
import type { QuietHours } from "@bindings/QuietHours";

export const NOTIFICATION_CHANNELS = ["in_app", "web_push", "ntfy", "email"] as const;
export type NotificationChannel = (typeof NOTIFICATION_CHANNELS)[number];

export const NOTIFICATION_KINDS = [
  "approval",
  "user_action",
  "user_action_blocking",
  "needs_input",
  "session_stuck",
  "system_alert",
  "inbox_stuck",
] as const;
export type NotificationKind = (typeof NOTIFICATION_KINDS)[number];

export const MAX_PUSH_PER_HOUR_DEFAULT = 20;
export const MAX_PUSH_PER_HOUR_LIMIT = 1000;
export const AFTER_MINUTES_MAX = 1440;

/** Stored settings are sparse: an absent channel or cell means the server default. */
export const DEFAULT_NOTIFICATIONS: NotificationSettings = {
  enabled: true,
  sound: true,
  channels: {},
  routes: {},
  quiet_hours: null,
  max_push_per_hour: MAX_PUSH_PER_HOUR_DEFAULT,
};

const isRecord = (v: unknown): v is Record<string, unknown> =>
  !!v && typeof v === "object" && !Array.isArray(v);

const HHMM = /^([01]\d|2[0-3]):[0-5]\d$/;

export function clampAfterMinutes(v: unknown): number {
  const n = typeof v === "number" && Number.isFinite(v) ? Math.round(v) : 5;
  return Math.min(AFTER_MINUTES_MAX, Math.max(1, n));
}

export function clampStage(v: unknown): NotificationStage | null {
  if (!isRecord(v)) return null;
  if (v.mode === "off" || v.mode === "now") return { mode: v.mode };
  if (v.mode === "after") return { mode: "after", minutes: clampAfterMinutes(v.minutes) };
  return null;
}

function clampChannels(v: unknown): Record<string, NotificationChannelPref> {
  const out: Record<string, NotificationChannelPref> = {};
  if (!isRecord(v)) return out;
  for (const ch of NOTIFICATION_CHANNELS) {
    const pref = v[ch];
    if (isRecord(pref) && typeof pref.enabled === "boolean") out[ch] = { enabled: pref.enabled };
  }
  return out;
}

function clampRoutes(v: unknown): Record<string, Record<string, NotificationStage>> {
  const out: Record<string, Record<string, NotificationStage>> = {};
  if (!isRecord(v)) return out;
  for (const kind of NOTIFICATION_KINDS) {
    const row = v[kind];
    if (!isRecord(row)) continue;
    const cells: Record<string, NotificationStage> = {};
    for (const ch of NOTIFICATION_CHANNELS) {
      const stage = clampStage(row[ch]);
      if (stage) cells[ch] = stage;
    }
    if (Object.keys(cells).length > 0) out[kind] = cells;
  }
  return out;
}

export function clampQuietHours(v: unknown): QuietHours | null {
  if (!isRecord(v)) return null;
  const { start, end, tz } = v;
  if (typeof start !== "string" || !HHMM.test(start)) return null;
  if (typeof end !== "string" || !HHMM.test(end)) return null;
  if (typeof tz !== "string" || !tz.trim()) return null;
  return { start, end, tz: tz.trim() };
}

export function clampMaxPushPerHour(v: unknown): number {
  if (typeof v !== "number" || !Number.isFinite(v)) return MAX_PUSH_PER_HOUR_DEFAULT;
  return Math.min(MAX_PUSH_PER_HOUR_LIMIT, Math.max(0, Math.round(v)));
}

export function mergeNotifications(v: unknown, displayNotifySound?: unknown): NotificationSettings {
  const raw = isRecord(v) ? v : {};
  const sound =
    typeof raw.sound === "boolean"
      ? raw.sound
      : typeof displayNotifySound === "boolean"
        ? displayNotifySound
        : DEFAULT_NOTIFICATIONS.sound;
  return {
    enabled: raw.enabled !== false,
    sound,
    channels: clampChannels(raw.channels),
    routes: clampRoutes(raw.routes),
    quiet_hours: clampQuietHours(raw.quiet_hours),
    max_push_per_hour: clampMaxPushPerHour(raw.max_push_per_hour),
  };
}

export function withStage(
  s: NotificationSettings,
  kind: string,
  channel: string,
  stage: NotificationStage,
): NotificationSettings {
  return {
    ...s,
    routes: { ...s.routes, [kind]: { ...(s.routes[kind] ?? {}), [channel]: stage } },
  };
}

export function withChannel(
  s: NotificationSettings,
  channel: string,
  enabled: boolean,
): NotificationSettings {
  return { ...s, channels: { ...s.channels, [channel]: { enabled } } };
}

const PUSH_DEFAULTS: Record<NotificationKind, NotificationStage> = {
  approval: { mode: "after", minutes: 5 },
  user_action: { mode: "off" },
  user_action_blocking: { mode: "after", minutes: 10 },
  needs_input: { mode: "after", minutes: 10 },
  session_stuck: { mode: "now" },
  system_alert: { mode: "now" },
  inbox_stuck: { mode: "now" },
};

/** Must match the server defaults: shown until `GET /notifications/settings` answers. */
export function fallbackStage(kind: string, channel: string): NotificationStage {
  if (channel === "in_app") return { mode: "now" };
  return PUSH_DEFAULTS[kind as NotificationKind] ?? { mode: "off" };
}

export const fallbackChannelEnabled = (channel: string): boolean => channel !== "email";

export function effectiveStage(
  stored: NotificationSettings,
  resolved: NotificationSettings | undefined,
  kind: string,
  channel: string,
): NotificationStage {
  return (
    stored.routes[kind]?.[channel] ??
    resolved?.routes[kind]?.[channel] ??
    fallbackStage(kind, channel)
  );
}

export function effectiveChannelEnabled(
  stored: NotificationSettings,
  resolved: NotificationSettings | undefined,
  channel: string,
): boolean {
  return (
    stored.channels[channel]?.enabled ??
    resolved?.channels[channel]?.enabled ??
    fallbackChannelEnabled(channel)
  );
}

export function browserTimeZone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  } catch {
    return "UTC";
  }
}
