import type { SoftEdit } from '../account-editor.logic';
import { isStaticCredential, providerFamily } from '$lib/providers';

export type PageId =
	| 'aliases'
	| 'limits'
	| 'ui'
	| 'privacy'
	| 'tools'
	| 'gateway'
	| 'models'
	| 'advanced';

/** Catalog group titles (settings keys are Title Case, env vars lowercase)
 *  routed to a drawer page; anything unmapped lands on Advanced. */
const GROUP_PAGE: Record<string, PageId> = {
	'UI & transcript': 'ui',
	Notifications: 'ui',
	thinking: 'ui',
	'Privacy & memory': 'privacy',
	telemetry: 'privacy',
	'Skills & workflows': 'tools',
	'Editing & safety': 'tools',
	'Remote control': 'tools',
	skills: 'tools',
	tokens: 'gateway',
	timeouts: 'gateway',
	context: 'gateway',
	cache: 'gateway',
	model: 'gateway',
	sessions: 'gateway'
};

export function groupPage(title: string): PageId {
	return GROUP_PAGE[title] ?? 'advanced';
}

export function pagesFor(kind: string): PageId[] {
	const family = providerFamily(kind);
	const out: PageId[] = ['aliases', 'limits'];
	if (family === 'anthropic') out.push('ui', 'privacy', 'tools');
	if (kind === 'fireworks' || isStaticCredential(kind)) out.push('models');
	out.push('gateway', 'advanced');
	return out;
}

const json = (v: unknown) => JSON.stringify(v ?? null);

export function diffCount(a: Record<string, unknown>, b: Record<string, unknown>): number {
	const keys = new Set([...Object.keys(a), ...Object.keys(b)]);
	let n = 0;
	for (const k of keys) if (json(a[k]) !== json(b[k])) n++;
	return n;
}

/** A cap and a bypass on the same window count as two changes. */
export function softFlat(edits: Record<string, SoftEdit>): Record<string, unknown> {
	const out: Record<string, unknown> = {};
	for (const [key, v] of Object.entries(edits)) {
		if (v.cap !== null) out[`${key}.cap`] = v.cap;
		if (v.capUsd !== null) out[`${key}.capUsd`] = v.capUsd;
		if (v.bypass !== null) out[`${key}.bypass`] = v.bypass;
	}
	return out;
}

export function settingsSlice(
	settings: Record<string, unknown>,
	keyNames: string[],
	envNames: string[]
): Record<string, unknown> {
	const out: Record<string, unknown> = {};
	for (const name of keyNames) if (settings[name] !== undefined) out[name] = settings[name];
	const env = settings.env;
	const envObj =
		env && typeof env === 'object' && !Array.isArray(env) ? (env as Record<string, unknown>) : {};
	for (const name of envNames) if (envObj[name] !== undefined) out[`env.${name}`] = envObj[name];
	return out;
}

/** Settings entries no page claims — the raw-JSON escape hatch on Advanced. */
export function looseSettings(
	settings: Record<string, unknown>,
	claimed: Set<string>
): [string, unknown][] {
	return Object.entries(settings).filter(([k]) => k !== 'env' && !claimed.has(k));
}
