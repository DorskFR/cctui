import type { SessionProfile } from '@bindings/SessionProfile';
import type { AccountPoolView } from '@bindings/AccountPoolView';
import type { AccountUsageEntry, OAuthAccount } from '$lib/queries';
import { headlinePct } from '$lib/components/molecules/usage-battery.logic';
import {
	adapterLabel,
	isCompatibleProvider,
	NO_ACCOUNT,
	poolName,
	poolValue,
	providerForAdapter
} from './options';
import type { Form } from './types';

/** The knobs a profile carries. The account pick is at most one of
 *  `account_id` / `pool_id` / `no_account`; none = Auto. `null` model /
 *  effort / permission mode = the harness or account default. */
export interface ProfileSpec {
	harness: string;
	account_id: string | null;
	pool_id: string | null;
	no_account: boolean;
	model_alias: string | null;
	effort: string | null;
	permission_mode: string | null;
}

export const SPEC_FIELDS = [
	'harness',
	'account_id',
	'pool_id',
	'no_account',
	'model_alias',
	'effort',
	'permission_mode'
] as const;

type SpecForm = Pick<
	Form,
	| 'adapter_id'
	| 'account'
	| 'account_provider'
	| 'model_claude'
	| 'model_codex'
	| 'model_account'
	| 'effort_claude'
	| 'effort_codex'
	| 'permission_mode'
>;

const blank = (v: string | null | undefined): string | null => (v?.trim() ? v.trim() : null);

export function specOf(p: SessionProfile): ProfileSpec {
	return {
		harness: p.harness,
		account_id: p.account_id,
		pool_id: p.pool_id,
		no_account: p.no_account,
		model_alias: blank(p.model_alias),
		effort: blank(p.effort),
		permission_mode: blank(p.permission_mode)
	};
}

/** Which form model field the harness reads: the account's own model list for
 *  a compatible endpoint, else the harness's native families. */
export function modelField(
	harness: string,
	account: OAuthAccount | undefined
): 'model_account' | 'model_codex' | 'model_claude' {
	const provider = providerForAdapter(account, harness);
	if (provider && isCompatibleProvider(provider.provider)) return 'model_account';
	return harness === 'codex' ? 'model_codex' : 'model_claude';
}

export const accountByName = (accounts: readonly OAuthAccount[], name: string) =>
	name ? accounts.find((a) => a.name === name) : undefined;
export const accountById = (accounts: readonly OAuthAccount[], id: string | null) =>
	id ? accounts.find((a) => a.id === id) : undefined;
const poolById = (pools: readonly AccountPoolView[], id: string | null) =>
	id ? pools.find((p) => p.id === id) : undefined;

/** The form's account picker value for a spec: '' Auto, the no-account
 *  sentinel, a pool value, or the account name. A pool or account that no
 *  longer exists falls back to Auto. */
export function accountPick(
	spec: ProfileSpec,
	accounts: readonly OAuthAccount[],
	pools: readonly AccountPoolView[]
): string {
	if (spec.no_account) return NO_ACCOUNT;
	const pool = poolById(pools, spec.pool_id);
	if (pool) return poolValue(pool.name);
	return accountById(accounts, spec.account_id)?.name ?? '';
}

/** The account's busiest window as a percentage, for the account picker hint. */
export function accountUsedPct(rows: readonly AccountUsageEntry[], accountId: string): number | null {
	const windows = rows.filter((r) => r.account === accountId).flatMap((r) => r.windows);
	return headlinePct(windows);
}

/** Read the profile knobs out of a form (the seed for the first profile). */
export function specFromForm(
	form: SpecForm,
	accounts: readonly OAuthAccount[],
	pools: readonly AccountPoolView[] = []
): ProfileSpec {
	const harness = form.adapter_id || 'claude-code';
	const pool = poolName(form.account);
	const account = pool === undefined ? accountByName(accounts, form.account) : undefined;
	return {
		harness,
		account_id: account?.id ?? null,
		pool_id: pool === undefined ? null : (pools.find((p) => p.name === pool)?.id ?? null),
		no_account: form.account === NO_ACCOUNT,
		model_alias: blank(form[modelField(harness, account)]),
		effort: blank(harness === 'codex' ? form.effort_codex : form.effort_claude),
		permission_mode: blank(form.permission_mode)
	};
}

/** The form with the profile knobs written over it; the rest (prompt, where,
 *  labels, env…) stays the caller's. */
export function applySpec<T extends SpecForm>(
	form: T,
	spec: ProfileSpec,
	accounts: readonly OAuthAccount[],
	pools: readonly AccountPoolView[] = []
): T {
	const account = accountById(accounts, spec.account_id);
	const out: T = {
		...form,
		adapter_id: spec.harness,
		account: accountPick(spec, accounts, pools),
		account_provider: providerForAdapter(account, spec.harness)?.provider ?? '',
		permission_mode: (spec.permission_mode ?? '') as T['permission_mode']
	};
	out[modelField(spec.harness, account)] = spec.model_alias ?? '';
	if (spec.harness === 'codex') out.effort_codex = spec.effort ?? '';
	else out.effort_claude = spec.effort ?? '';
	return out;
}

/** How many knobs differ between two specs (the adjust panel's "N changes"). */
export function specChanges(a: ProfileSpec, b: ProfileSpec): number {
	return SPEC_FIELDS.filter((f) => (a[f] ?? null) !== (b[f] ?? null)).length;
}

export function sameSpec(a: ProfileSpec, b: ProfileSpec): boolean {
	return specChanges(a, b) === 0;
}

/** The one-line summary under a profile name:
 *  "Claude Code · 🐼 personal · Fable · medium · Yolo". */
export function specChain(
	spec: ProfileSpec,
	accounts: readonly OAuthAccount[],
	pools: readonly AccountPoolView[],
	labels: {
		auto: string;
		noAccount: string;
		defaultModel: string;
		defaultEffort: string;
		defaultMode: string;
	},
	modelLabel: (harness: string, alias: string) => string = (_h, alias) => alias
): string {
	const account = accountById(accounts, spec.account_id);
	const pool = poolById(pools, spec.pool_id);
	const accountText = spec.no_account
		? labels.noAccount
		: pool
			? pool.name
			: account
				? `${account.emoji ? `${account.emoji} ` : ''}${account.name}`
				: labels.auto;
	const mode = spec.permission_mode
		? spec.permission_mode[0].toUpperCase() + spec.permission_mode.slice(1)
		: labels.defaultMode;
	return [
		adapterLabel(spec.harness),
		accountText,
		spec.model_alias ? modelLabel(spec.harness, spec.model_alias) : labels.defaultModel,
		spec.effort ?? labels.defaultEffort,
		mode
	].join(' · ');
}

/** `base`, else `base 2`, `base 3`… — whatever the caller's list lacks. */
export function uniqueProfileName(base: string, existing: readonly string[]): string {
	const taken = new Set(existing.map((n) => n.toLowerCase()));
	if (!taken.has(base.toLowerCase())) return base;
	for (let i = 2; ; i++) {
		const candidate = `${base} ${i}`;
		if (!taken.has(candidate.toLowerCase())) return candidate;
	}
}

/** Which profile the panel opens on: the machine's last-used one when it
 *  still exists, else the first. */
export function initialProfile(
	profiles: readonly SessionProfile[],
	lastUsedId: string | null
): SessionProfile | null {
	return profiles.find((p) => p.id === lastUsedId) ?? profiles[0] ?? null;
}
