import type { Indicator, ProviderStatus } from "$lib/queries";

/** Severity order, worst last. `unknown` is not a severity — it never ranks. */
const ORDER: Indicator[] = ["none", "minor", "major", "critical"];

export const isDegraded = (i: Indicator) => i === "minor" || i === "major" || i === "critical";

/** Only the families worth showing: healthy and unknown both say nothing. */
export const degraded = (list: ProviderStatus[] | undefined): ProviderStatus[] =>
  (list ?? []).filter((s) => isDegraded(s.indicator));

/** The worst indicator across the given families, or `null` when none is degraded. */
export function worstIndicator(list: ProviderStatus[] | undefined): Indicator | null {
  let worst: Indicator | null = null;
  for (const s of degraded(list)) {
    if (worst === null || ORDER.indexOf(s.indicator) > ORDER.indexOf(worst)) worst = s.indicator;
  }
  return worst;
}

/** Tone token for an indicator: `minor` is a warning, worse is a danger. */
export const indicatorTone = (i: Indicator | null): "warn" | "danger" | null =>
  i === "minor" ? "warn" : i === "major" || i === "critical" ? "danger" : null;

/** Display name for a polled family. */
export const familyLabel = (family: string) =>
  family === "anthropic" ? "Claude" : family === "openai" ? "Codex" : family;

/** The affected component names, capped so a badge cannot grow without bound. */
export function componentNames(status: ProviderStatus, max = 3): string[] {
  return status.components.slice(0, max).map((c) => c.name);
}

/** The status reading for one account provider id, and only when it is degraded.
 *  Mirrors the server: a `*-compatible` credential points at somebody else's
 *  endpoint, so it never inherits a first-party incident. */
export function statusForProvider(
  list: ProviderStatus[] | undefined,
  provider: string,
): ProviderStatus | null {
  const family = provider === "anthropic" ? "anthropic" : provider === "openai" ? "openai" : null;
  if (!family) return null;
  return degraded(list).find((s) => s.family === family) ?? null;
}
