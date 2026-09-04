import { browser } from "$app/environment";
import { goto } from "$app/navigation";
import { m } from "$lib/paraglide/messages";
import type { SessionListItem } from "@bindings/SessionListItem";

const KEY_ENABLED = "cctui_notify_enabled";
const KEY_SOUND = "cctui_notify_sound";
const BRAND = "cctui";

/**
 * Browser-notification + sound for sessions that need the user's input.
 * List-driven: `reconcile()` is fed the set of sessions with
 * `attention === 'needs_input'` and fires one notification per session per
 * episode (dedupe by id; re-arms once the session stops needing input).
 *
 * The title badge (`(N) cctui`) updates regardless of notification
 * permission, so it degrades gracefully when notifications are denied or the
 * page isn't a secure context.
 */
class Notifier {
  permission = $state<NotificationPermission>("default");
  enabled = $state(false);
  sound = $state(true);

  /** Session whose drawer is currently open — used to suppress self-noise. */
  openSessionId: string | null = null;
  /** Set by a notification click; the sessions page opens this drawer. */
  pendingOpen = $state<string | null>(null);

  private notified = new Set<string>();
  /** Last badge count, replayed when the base title changes. */
  private badgeCount = 0;
  /** Server-wide instance name (`/version.instance_name`); part of the tab title. */
  private instanceName: string | null = null;
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
   * Diff the set of sessions needing input against what we've already
   * notified, fire for newly-blocked sessions, and keep the title badge in
   * sync. Cheap to call on every list change.
   */
  reconcile(needing: SessionListItem[]) {
    if (!browser) return;
    const ids = new Set(needing.map((s) => s.id));
    // Re-arm sessions that stopped needing input, so the next episode notifies.
    for (const id of [...this.notified])
      if (!ids.has(id)) this.notified.delete(id);

    this.updateBadge(needing.length);

    if (!this.enabled || this.permission !== "granted") return;
    for (const s of needing) {
      if (this.notified.has(s.id)) continue;
      this.notified.add(s.id);
      // Don't notify for a session you're already staring at.
      if (document.visibilityState === "visible" && this.openSessionId === s.id)
        continue;
      this.fire(s);
    }
  }

  /** Base tab title: `cctui`, or `cctui (NAME)` when the admin labelled the instance. */
  baseTitle(): string {
    return this.instanceName ? `${BRAND} (${this.instanceName})` : BRAND;
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

  private fire(s: SessionListItem) {
    try {
      const n = new Notification(m.notify_needs_input_title(), {
        body: s.last_message_text
          ? `${this.label(s)}\n${s.last_message_text}`
          : this.label(s),
        tag: s.id,
      });
      n.onclick = () => {
        window.focus();
        this.pendingOpen = s.id;
        void goto("/sessions");
        n.close();
      };
    } catch {
      /* construction can throw on some platforms; ignore */
    }
    if (this.sound) this.playSound();
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
