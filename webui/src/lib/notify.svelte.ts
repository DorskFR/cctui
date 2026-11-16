import { browser } from "$app/environment";
import { goto } from "$app/navigation";
import { m } from "$lib/paraglide/messages";
import type { SessionListItem } from "@bindings/SessionListItem";
import type { Notification as Notice } from "@bindings/Notification";
import { isUnread } from "./notifications.svelte";
import { freshResults } from "./outcome";

const KEY_ENABLED = "cctui_notify_enabled";
const KEY_SOUND = "cctui_notify_sound";
const BRAND = "cctui";

/**
 * The page-local renderer of the notification service: `announce()` turns each
 * live notification into a browser alert and closes it once the notice is read
 * or answered anywhere. The title badge (`(N) cctui`) is the server's unread
 * count and updates whatever the notification permission.
 */
class Notifier {
  permission = $state<NotificationPermission>("default");
  enabled = $state(false);
  sound = $state(true);

  /** Sessions the user currently has open — the drawer and any tiles. Reads
   *  as a ref count so the same session held twice survives one close. */
  #open = new Map<string, number>();
  /** Reactive mirror of the held set, for views that zero unread badges. */
  openSessionIds = $state<ReadonlySet<string>>(new Set());
  /** Set by a notification click; the sessions page opens this drawer. */
  pendingOpen = $state<string | null>(null);

  private notified = new Set<string>();
  /** Notice id -> repeat count already alerted. */
  private alerted = new Map<string, number>();
  private shown = new Map<string, Notification>();
  /** Result outcome ids already seen; the first list load only seeds it. */
  private announced: Set<string> | null = null;
  /** Last badge count, replayed when the base title changes. */
  private badgeCount = 0;
  /** Server-wide instance name (`/version.instance_name`); part of the tab title. */
  private instanceName: string | null = null;
  /** The open conversation's display name; leads the tab title. */
  private sessionTitle: string | null = null;
  private audioCtx: AudioContext | null = null;

  constructor() {
    if (browser) {
      this.permission = this.supported ? Notification.permission : "denied";
      this.enabled =
        localStorage.getItem(KEY_ENABLED) === "1" &&
        this.permission === "granted";
      this.sound = localStorage.getItem(KEY_SOUND) !== "0";
    }
  }

  get supported(): boolean {
    return browser && "Notification" in window;
  }

  private async requestPermission(): Promise<boolean> {
    if (!this.supported) return false;
    try {
      this.permission = await Notification.requestPermission();
    } catch {
      this.permission = Notification.permission;
    }
    return this.permission === "granted";
  }

  /** Turn notifications on, requesting permission if needed. Returns success. */
  async enable(): Promise<boolean> {
    const ok = await this.requestPermission();
    this.enabled = ok;
    if (browser) localStorage.setItem(KEY_ENABLED, ok ? "1" : "0");
    return ok;
  }

  disable() {
    this.enabled = false;
    if (browser) localStorage.setItem(KEY_ENABLED, "0");
  }

  setSound(on: boolean) {
    this.sound = on;
    if (browser) localStorage.setItem(KEY_SOUND, on ? "1" : "0");
  }

  /** Reflect the server-persisted display settings on load. Never requests
   *  permission: `enabled` only sticks while the browser still grants it. */
  applyPersisted(enabled: boolean, sound: boolean) {
    if (!browser) return;
    this.setSound(sound);
    const on = enabled && this.permission === "granted";
    this.enabled = on;
    localStorage.setItem(KEY_ENABLED, on ? "1" : "0");
  }

  /**
   * One alert per needs-input episode for sessions no notification speaks
   * for: prompts the server does not turn into notifications yet.
   */
  reconcile(needing: SessionListItem[], covered: ReadonlySet<string> = new Set()) {
    if (!browser) return;
    const ids = new Set(needing.map((s) => s.id));
    // Re-arm sessions that stopped needing input, so the next episode notifies.
    for (const id of [...this.notified])
      if (!ids.has(id)) this.notified.delete(id);

    if (!this.enabled || this.permission !== "granted") return;
    for (const s of needing) {
      if (this.notified.has(s.id)) continue;
      this.notified.add(s.id);
      if (covered.has(s.id)) continue;
      // Don't notify for a session you're already staring at.
      if (document.visibilityState === "visible" && this.#open.has(s.id)) continue;
      this.fire(s, m.notify_needs_input_title(), s.last_message_text);
    }
  }

  /** A live notification frame: alert on first sight and on each collapse
   *  repeat; close the banner once it is no longer unread. */
  announce(n: Notice) {
    if (!browser) return;
    if (!isUnread(n)) {
      this.alerted.delete(n.id);
      this.shown.get(n.id)?.close();
      this.shown.delete(n.id);
      return;
    }
    if ((this.alerted.get(n.id) ?? 0) >= n.repeat_count) return;
    this.alerted.set(n.id, n.repeat_count);
    if (!this.enabled || this.permission !== "granted") return;
    if (document.visibilityState === "visible" && n.session_id && this.#open.has(n.session_id)) return;
    const shown = this.show(n.title, n.body ?? undefined, n.collapse_key ?? n.id, n.session_id);
    if (shown) this.shown.set(n.id, shown);
  }

  setUnread(count: number) {
    this.updateBadge(count);
  }

  /** Fire once per `result` outcome that lands while the page is open. */
  reconcileCompleted(sessions: SessionListItem[] | undefined) {
    if (!browser || !sessions) return;
    if (!this.announced) {
      this.announced = new Set();
      freshResults(this.announced, sessions);
      return;
    }
    const fresh = freshResults(this.announced, sessions);
    if (!this.enabled || this.permission !== "granted") return;
    for (const s of fresh) {
      if (document.visibilityState === "visible" && this.#open.has(s.id)) continue;
      this.fire(s, m.notify_completed_title(), s.latest_outcome?.headline);
    }
  }

  /** Register a session as on screen; call the returned teardown when its pane
   *  closes. */
  holdOpen(id: string): () => void {
    const n = this.#open.get(id) ?? 0;
    this.#open.set(id, n + 1);
    this.openSessionIds = new Set(this.#open.keys());
    return () => {
      const left = (this.#open.get(id) ?? 0) - 1;
      if (left > 0) this.#open.set(id, left);
      else this.#open.delete(id);
      this.openSessionIds = new Set(this.#open.keys());
    };
  }

  /** Base tab title: `cctui`, or `cctui (NAME)` when the admin labelled the
   *  instance, behind the open session's name. */
  baseTitle(): string {
    const brand = this.instanceName ? `${BRAND} (${this.instanceName})` : BRAND;
    return this.sessionTitle ? `${this.sessionTitle} · ${brand}` : brand;
  }

  setSessionTitle(title: string | null | undefined) {
    const next = title?.trim() || null;
    if (next === this.sessionTitle) return;
    this.sessionTitle = next;
    this.updateBadge(this.badgeCount);
  }

  /** Header feeds this from `/version`; re-renders the title with the current badge. */
  setInstanceName(name: string | null | undefined) {
    const next = name?.trim() || null;
    if (next === this.instanceName) return;
    this.instanceName = next;
    this.updateBadge(this.badgeCount);
  }

  private updateBadge(n: number) {
    this.badgeCount = n;
    const base = this.baseTitle();
    if (browser) document.title = n > 0 ? `(${n}) ${base}` : base;
  }

  private label(s: SessionListItem): string {
    return (
      s.name ||
      s.working_dir?.split("/").filter(Boolean).pop() ||
      s.id.slice(0, 8)
    );
  }

  private fire(s: SessionListItem, title: string, detail: string | null | undefined) {
    this.show(title, detail ? `${this.label(s)}\n${detail}` : this.label(s), s.id, s.id);
  }

  private show(
    title: string,
    body: string | undefined,
    tag: string,
    sessionId: string | null,
  ): Notification | null {
    let n: Notification | null = null;
    try {
      const shown = new Notification(title, { body, tag });
      shown.onclick = () => {
        window.focus();
        if (sessionId) {
          this.pendingOpen = sessionId;
          if (!/^\/sessions(\/|$)/.test(location.pathname)) void goto("/sessions");
        }
        shown.close();
      };
      n = shown;
    } catch {
      /* construction can throw on some platforms; ignore */
    }
    if (this.sound) this.playSound();
    return n;
  }

  /** Short WebAudio "ping" — no bundled asset, gated behind the user opt-in. */
  playSound() {
    if (!browser) return;
    try {
      const Ctx =
        window.AudioContext ??
        (window as unknown as { webkitAudioContext?: typeof AudioContext })
          .webkitAudioContext;
      if (!Ctx) return;
      this.audioCtx ??= new Ctx();
      const ctx = this.audioCtx;
      if (ctx.state === "suspended") void ctx.resume();
      const t = ctx.currentTime;
      const osc = ctx.createOscillator();
      const gain = ctx.createGain();
      osc.type = "sine";
      osc.frequency.setValueAtTime(880, t);
      osc.frequency.setValueAtTime(660, t + 0.12);
      gain.gain.setValueAtTime(0.0001, t);
      gain.gain.exponentialRampToValueAtTime(0.18, t + 0.01);
      gain.gain.exponentialRampToValueAtTime(0.0001, t + 0.3);
      osc.connect(gain);
      gain.connect(ctx.destination);
      osc.start(t);
      osc.stop(t + 0.32);
    } catch {
      /* autoplay / context issues — best-effort */
    }
  }
}

export const notify = new Notifier();
