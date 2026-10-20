// Pure derivations over the harness table: which harnesses a picker offers,
// what a session's harness can do, which brand mark it wears and which
// permission modes it can express. The table defaults to the shipped
// `HARNESSES`; the store in `$lib/harnesses.svelte` hands in the served one.
import type { HarnessCapabilities } from '@bindings/HarnessCapabilities';
import type { HarnessDescriptor } from '@bindings/HarnessDescriptor';
import type { PermissionMode } from '@bindings/PermissionMode';
import type { ProviderFamily } from '@bindings/ProviderFamily';
import { HARNESSES, PERMISSION_MODES, providerInfo } from '$lib/domainTables';

export type HarnessTable = readonly HarnessDescriptor[];

export const NO_CAPABILITIES: HarnessCapabilities = {
	fork: false,
	rename: false,
	resume: false,
	set_model: false,
	live_view: false,
	attach: false,
	mid_chat_files: false,
	child_spawn: false
};

/** The row whose id is exactly `id`. */
export const harnessById = (
	id: string | null | undefined,
	table: HarnessTable = HARNESSES
): HarnessDescriptor | undefined => (id ? table.find((h) => h.id === id) : undefined);

/** The row an adapter id resolves to: the id itself or a dash-suffixed variant
 *  of it (`codex-app-server`). Anything else resolves to nothing. */
export const harnessForAdapter = (
	adapterId: string | null | undefined,
	table: HarnessTable = HARNESSES
): HarnessDescriptor | undefined => {
	const id = (adapterId ?? '').trim();
	if (!id) return undefined;
	return table.find((h) => id === h.id || id.startsWith(`${h.id}-`));
};

export const harnessLabel = (id: string, table: HarnessTable = HARNESSES): string =>
	harnessById(id, table)?.label ?? id;

/** An unknown harness can do nothing the UI would offer a control for. */
export const harnessCapabilities = (
	adapterId: string | null | undefined,
	table: HarnessTable = HARNESSES
): HarnessCapabilities => harnessForAdapter(adapterId, table)?.capabilities ?? NO_CAPABILITIES;

/** The harnesses a spawn picker lists: the ids `enabled` names when a machine
 *  reports its own set, else every row that runs by default. */
export const pickableHarnesses = (
	table: HarnessTable = HARNESSES,
	enabled?: readonly string[] | null
): HarnessDescriptor[] =>
	table.filter((h) => (enabled ? enabled.includes(h.id) : h.default_enabled));

/** The postures a harness can express. An unknown harness keeps the full list,
 *  so a free-text pick is never left without a posture. */
export const harnessPermissionModes = (
	harnessId: string | null | undefined,
	table: HarnessTable = HARNESSES
): PermissionMode[] => harnessById(harnessId, table)?.permission_modes ?? PERMISSION_MODES;

export type BrandMark = ProviderFamily | 'neutral';

/** Which mark an adapter or provider wears. A provider resolves through the
 *  provider table, an adapter through the harness table; anything unknown, or
 *  nothing at all, wears the neutral glyph rather than another vendor's logo. */
export const brandMark = (
	{ adapter, provider }: { adapter?: string | null; provider?: string | null },
	table: HarnessTable = HARNESSES
): BrandMark => {
	if (provider != null) return providerInfo(provider)?.family ?? 'neutral';
	return harnessForAdapter(adapter, table)?.family ?? 'neutral';
};

/** The harness a provider credential runs: the first row of the provider's
 *  family. */
export const harnessForProvider = (
	provider: string,
	table: HarnessTable = HARNESSES
): string | undefined => {
	const family = providerInfo(provider)?.family ?? 'anthropic';
	return table.find((h) => h.family === family)?.id;
};

/** The harnesses a set of provider credentials can run, in table order. */
export const harnessesForProviders = (
	providers: readonly string[],
	table: HarnessTable = HARNESSES
): string[] => {
	const families = new Set(providers.map((p) => providerInfo(p)?.family ?? 'anthropic'));
	return table.filter((h) => families.has(h.family)).map((h) => h.id);
};

/** The accent a harness card or icon wears, by family; neutral for anything
 *  the table does not know. */
export const familyAccent = (mark: BrandMark): string =>
	mark === 'anthropic'
		? 'var(--c-amber)'
		: mark === 'openai'
			? 'var(--c-blue)'
			: mark === 'fireworks'
				? 'var(--c-violet, var(--c-blue))'
				: 'var(--c-fg-muted, currentColor)';
