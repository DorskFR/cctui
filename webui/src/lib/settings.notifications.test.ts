// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { mergeDefaults, settings } from "./settings.svelte";
import { auth } from "./auth.svelte";
import {
  DEFAULT_NOTIFICATIONS,
  effectiveChannelEnabled,
  effectiveStage,
  mergeNotifications,
} from "./notificationPrefs";
import type { NotificationSettings } from "@bindings/NotificationSettings";

beforeEach(() => {
  auth.isAuthed = false;
  localStorage.clear();
});

afterEach(() => {
  localStorage.clear();
});

describe("notifications settings mapping", () => {
  it("defaults to on, sound on, sparse maps, no quiet hours, cap 20", () => {
    expect(mergeDefaults({}).notifications).toEqual(DEFAULT_NOTIFICATIONS);
    expect(DEFAULT_NOTIFICATIONS).toEqual({
      enabled: true,
      sound: true,
      channels: {},
      routes: {},
      quiet_hours: null,
      max_push_per_hour: 20,
    });
  });

  it("migrates display.notifySound into notifications.sound and drops the old key", () => {
    const loaded = mergeDefaults({ display: { notifySound: false } } as Record<string, unknown>);
    expect(loaded.notifications.sound).toBe(false);
    expect(loaded.display.notifySound).toBeUndefined();
    expect(JSON.parse(JSON.stringify(loaded)).display).not.toHaveProperty("notifySound");
  });

  it("prefers notifications.sound over the legacy display value", () => {
    const loaded = mergeDefaults({
      display: { notifySound: false },
      notifications: { sound: true },
    } as Record<string, unknown>);
    expect(loaded.notifications.sound).toBe(true);
  });

  it("clamps unknown channels, kinds, modes and out-of-range numbers", () => {
    const n = mergeNotifications({
      enabled: false,
      channels: { ntfy: { enabled: false }, pigeon: { enabled: true }, email: "yes" },
      routes: {
        approval: { ntfy: { mode: "after", minutes: 99999 }, in_app: { mode: "loud" } },
        made_up: { ntfy: { mode: "now" } },
        needs_input: { web_push: { mode: "after", minutes: 0 } },
      },
      quiet_hours: { start: "22:00", end: "07:30", tz: " Europe/Paris " },
      max_push_per_hour: -3,
    });
    expect(n).toEqual({
      enabled: false,
      sound: true,
      channels: { ntfy: { enabled: false } },
      routes: {
        approval: { ntfy: { mode: "after", minutes: 1440 } },
        needs_input: { web_push: { mode: "after", minutes: 1 } },
      },
      quiet_hours: { start: "22:00", end: "07:30", tz: "Europe/Paris" },
      max_push_per_hour: 0,
    });
  });

  it("drops malformed quiet hours", () => {
    expect(mergeNotifications({ quiet_hours: { start: "25:00", end: "07:00", tz: "UTC" } }).quiet_hours).toBeNull();
    expect(mergeNotifications({ quiet_hours: { start: "22:00", end: "07:00", tz: "" } }).quiet_hours).toBeNull();
  });

  it("setters persist sparse overrides under the notifications key", () => {
    settings.setNotifications(DEFAULT_NOTIFICATIONS);
    settings.setNotificationChannel("ntfy", false);
    settings.setNotificationStage("approval", "web_push", { mode: "after", minutes: 15 });
    settings.setNotifySound(false);
    const cached = JSON.parse(localStorage.getItem("cctui_settings") ?? "{}");
    expect(cached.notifications).toEqual({
      ...DEFAULT_NOTIFICATIONS,
      sound: false,
      channels: { ntfy: { enabled: false } },
      routes: { approval: { web_push: { mode: "after", minutes: 15 } } },
    });
    expect(cached.display).not.toHaveProperty("notifySound");
  });

  it("an override wins over the resolved view, which wins over the fallback", () => {
    const resolved: NotificationSettings = {
      ...DEFAULT_NOTIFICATIONS,
      channels: { email: { enabled: true } },
      routes: { approval: { ntfy: { mode: "now" } } },
    };
    const stored = mergeNotifications({ routes: { approval: { web_push: { mode: "off" } } } });
    expect(effectiveStage(stored, resolved, "approval", "web_push")).toEqual({ mode: "off" });
    expect(effectiveStage(stored, resolved, "approval", "ntfy")).toEqual({ mode: "now" });
    expect(effectiveStage(stored, undefined, "approval", "email")).toEqual({ mode: "after", minutes: 5 });
    expect(effectiveStage(stored, undefined, "user_action", "in_app")).toEqual({ mode: "now" });
    expect(effectiveStage(stored, undefined, "user_action", "ntfy")).toEqual({ mode: "off" });
    expect(effectiveChannelEnabled(stored, resolved, "email")).toBe(true);
    expect(effectiveChannelEnabled(stored, undefined, "email")).toBe(false);
    expect(effectiveChannelEnabled(stored, undefined, "ntfy")).toBe(true);
  });
});
