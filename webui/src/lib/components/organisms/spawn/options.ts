// Shared spawn-form option lists, extracted from SpawnModal (no behavior
// change). Used by the machine + dispatch field groups for their model/effort
// selectors and permission-mode picker.
import type { PermissionMode } from '@bindings/PermissionMode';
import type { AccountProvider, OAuthAccount } from '$lib/queries';

export const modes: { v: PermissionMode; label: string; hint: string }[] = [
	{ v: 'ask', label: 'Ask', hint: 'Prompt on every action' },
	{ v: 'auto', label: 'Auto', hint: 'Auto-apply, sandbox on' },
	{ v: 'yolo', label: 'Yolo', hint: 'No prompts, full access' },
	{ v: 'whip', label: 'Whip 🐎', hint: 'Yolo + no asking, no stalling' }
];

// Per-adapter effort levels, index 0 = "" (adapter default). Claude and codex
// expose different reasoning-effort vocabularies.
export const claudeEfforts = ['', 'low', 'medium', 'high', 'xhigh', 'max'];
export const codexEfforts = ['', 'low', 'medium', 'high', 'xhigh'];

// Model families per adapter (CCT-274). `''` = the adapter's own default.
// claude resolves the family alias (opus/sonnet/haiku/fable) to a concrete
// model; codex takes the model slug directly.
export const claudeModels = [
	{ v: '', label: 'Default' },
	{ v: 'haiku', label: 'Haiku' },
	{ v: 'sonnet', label: 'Sonnet' },
	{ v: 'opus', label: 'Opus' },
	{ v: 'fable', label: 'Fable' }
];
export const codexModels = [
	{ v: '', label: 'Default' },
	{ v: 'gpt-5.5-codex', label: 'GPT-5.5 Codex' },
	{ v: 'gpt-5.4-codex', label: 'GPT-5.4 Codex' }
];

// Annotate native-family options with the per-account alias target (CCT-406)
// so the picker reads e.g. "Opus (claude-opus-4-8[1m])" instead of a bare
// "Opus" — making it obvious which concrete model the family resolves to (and
// that the alias is in effect) for the selected account. A no-op when the
// account has no matching alias for that family.
export const withAliasTargets = (
	models: { v: string; label: string }[],
	aliases: Record<string, string> | null | undefined
): { v: string; label: string }[] =>
	models.map((m) => {
		const target = m.v ? aliases?.[m.v]?.trim() : undefined;
		return target ? { v: m.v, label: `${m.label} (${target})` } : m;
	});

// The harness/adapter a provider credential runs (CCT-399): anything in the
// openai family runs Codex; everything else (anthropic / anthropic-compatible)
// runs Claude Code. Mirrors the server's `Family::from_provider`.
export const adapterForProvider = (provider: string): Adapter =>
	provider.includes('openai') ? 'codex' : 'claude-code';

export type Adapter = 'claude-code' | 'codex';
// Stable field order (CCT-404): the harness cards never reorder.
export const allAdapters: Adapter[] = ['claude-code', 'codex'];

// Provider-family union of an account identity (CCT-562): the harnesses its
// credentials can run, in stable order. An account holding anthropic+openai
// providers offers both; a single-provider account offers one.
export const accountAdapters = (a: OAuthAccount): Adapter[] => {
	const families = new Set(a.providers.map((p) => adapterForProvider(p.provider)));
	return allAdapters.filter((ad) => families.has(ad));
};

// The provider credential backing a harness on this account, if any.
export const providerForAdapter = (
	a: OAuthAccount | undefined,
	adapter: string
): AccountProvider | undefined => a?.providers.find((p) => adapterForProvider(p.provider) === adapter);

// The harness in effect: the user's pick when no account is chosen or the
// account offers it, else the first harness the account can run.
export const effectiveAdapterFor = (a: OAuthAccount | undefined, adapterId: string): string => {
	if (!a) return adapterId;
	const allowed = accountAdapters(a);
	return allowed.includes(adapterId as Adapter) ? adapterId : (allowed[0] ?? adapterId);
};

// A compatible-endpoint account carries its own model list; a native
// subscription account uses the harness's native families.
export const isCompatibleProvider = (provider: string): boolean =>
	provider.endsWith('-compatible');

export const adapterLabel = (adapter: string): string =>
	adapter === 'codex' ? 'Codex' : 'Claude Code';
