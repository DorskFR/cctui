// Shared spawn-form option lists, extracted from SpawnModal (no behavior
// change). Used by the machine + dispatch field groups for their model/effort
// selectors and permission-mode picker.
import type { PermissionMode } from '@bindings/PermissionMode';
import type { AccountProvider, OAuthAccount } from '$lib/queries';
import { m } from '$lib/paraglide/messages';

export const modes: { v: PermissionMode; label: string; hint: string }[] = [
	{
		v: 'ask',
		get label() {
			return m.spawn_mode_ask_label();
		},
		get hint() {
			return m.spawn_mode_ask_hint();
		}
	},
	{
		v: 'auto',
		get label() {
			return m.spawn_mode_auto_label();
		},
		get hint() {
			return m.spawn_mode_auto_hint();
		}
	},
	{
		v: 'yolo',
		get label() {
			return m.spawn_mode_yolo_label();
		},
		get hint() {
			return m.spawn_mode_yolo_hint();
		}
	},
	{
		v: 'whip',
		get label() {
			return m.spawn_mode_whip_label();
		},
		get hint() {
			return m.spawn_mode_whip_hint();
		}
	}
];

export {
	codexModels,
	codexEfforts,
	codexModelsFor,
	codexEffortsFor,
	claudeModels,
	claudeEfforts
} from '$lib/harnessModels';

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

// Whether the account can back this harness (has a provider in its family).
// Drives the CCT-581 validation that replaced the silent adapter swap: a named
// account that can't back the picked harness blocks the spawn with an explicit
// error instead of quietly submitting the account's own family. No account =
// always valid (Default/no-account runs any harness).
export const accountBacksAdapter = (a: OAuthAccount | undefined, adapter: string): boolean =>
	!a || accountAdapters(a).includes(adapter as Adapter);

// Sentinel account value for an explicit unbound "no account" spawn (CCT-582):
// distinct from '' (Auto — let the server bind the single matching account) and
// from a named account. Kept out of the account-name space so it can never
// collide with a real account name.
export const NO_ACCOUNT = '\x00no-account';

// A compatible-endpoint account carries its own model list; a native
// subscription account uses the harness's native families.
export const isCompatibleProvider = (provider: string): boolean =>
	provider.endsWith('-compatible');

export const adapterLabel = (adapter: string): string =>
	adapter === 'codex' ? 'Codex' : 'Claude Code';
