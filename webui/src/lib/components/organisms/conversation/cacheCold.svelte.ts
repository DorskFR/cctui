// Cold-cache clock for the composer's Send button. Once the prompt cache
// lapses the next send re-writes the whole context to cache (an expensive
// "burst"). The window runs from the newest agent activity: the server's last
// billed turn, or a later agent event this pane saw stream in. Send routes off
// the same state the button shows.
import { now as clockNow } from "$lib/clock.svelte";
import { cacheTtlMs } from "./cacheTtl";

// Final-minute countdown window.
export const COLD_WARN_MS = 60 * 1000;

export interface CacheColdOpts {
  adapterId: () => string | null;
  model: () => string | null;
  lastActivityAt: () => string | null;
  /** Newest agent event seen live, in ms. */
  lastAgentAt?: () => number | null;
  /** Reactive wall clock keyed by tick period; injectable for tests. */
  clock?: (periodMs: number) => number;
}

export class CacheColdClock {
  #o: CacheColdOpts;
  #clock: (periodMs: number) => number;

  constructor(o: CacheColdOpts) {
    this.#o = o;
    this.#clock = o.clock ?? clockNow;
  }

  ttlMs = $derived.by(() => cacheTtlMs(this.#o.adapterId(), this.#o.model()));
  lastActivityMs = $derived.by(() => {
    const at = this.#o.lastActivityAt();
    const server = at ? new Date(at).getTime() : null;
    const live = this.#o.lastAgentAt?.() ?? null;
    if (server === null) return live;
    return live === null ? server : Math.max(server, live);
  });
  // A lazy 15s tick flips the button cold; 1s only around the countdown.
  #slowNow = $derived.by(() => this.#clock(15_000));
  #nearCold = $derived.by(() => {
    if (this.lastActivityMs === null) return false;
    const left = this.ttlMs - (this.#slowNow - this.lastActivityMs);
    return left > -15_000 && left <= COLD_WARN_MS + 15_000;
  });
  now = $derived.by(() =>
    this.#nearCold ? this.#clock(1_000) : this.#slowNow,
  );
  cold = $derived.by(
    () =>
      this.lastActivityMs !== null &&
      this.now - this.lastActivityMs > this.ttlMs,
  );
  msUntilCold = $derived(
    this.lastActivityMs === null
      ? null
      : this.ttlMs - (this.now - this.lastActivityMs),
  );
  imminent = $derived.by(
    () =>
      this.msUntilCold !== null &&
      this.msUntilCold > 0 &&
      this.msUntilCold <= COLD_WARN_MS,
  );
  countdownSecs = $derived(
    this.imminent ? Math.ceil((this.msUntilCold as number) / 1000) : null,
  );
}
