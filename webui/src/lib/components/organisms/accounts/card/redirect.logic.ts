export type RedirectTarget = { id: string; name: string; families: string[] };
export type RedirectChip = { id: string; family: string; targetName: string; until: string | null };

/** Families the target can take over: ones this account runs and no rule redirects yet. */
export function openFamilies(
	target: RedirectTarget,
	ownFamilies: readonly string[],
	redirects: readonly RedirectChip[]
): string[] {
	const mine = new Set(ownFamilies);
	const ruled = new Set(redirects.map((r) => r.family));
	return target.families.filter((f) => mine.has(f) && !ruled.has(f));
}

export function openRedirectTargets(
	targets: readonly RedirectTarget[],
	ownFamilies: readonly string[],
	redirects: readonly RedirectChip[]
): RedirectTarget[] {
	return targets.filter((t) => openFamilies(t, ownFamilies, redirects).length > 0);
}

/** Full local date and time for a hover title; undefined when absent or unparseable. */
export function fullTime(value: string | null | undefined): string | undefined {
	if (!value) return undefined;
	const d = new Date(value);
	return Number.isNaN(d.getTime()) ? undefined : d.toLocaleString();
}
