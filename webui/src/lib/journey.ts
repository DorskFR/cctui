import { goto } from '$app/navigation';
import type { QueryClient } from '@tanstack/svelte-query';
import type { IR, Journey } from '@dorsk/journey';
import {
	DONE_PREFIX,
	type JourneyApi,
	type JourneyStorage,
	mount,
	PROGRESS_KEY,
	type RunResult
} from '@dorsk/journey/runtime';
import journeys from './journeys.generated.json';
import { createProbes, isLive, type Probes } from './journeys/probes';
import { endpoints } from './queries/endpoints';
import { qk } from './queries/keys';
import { settings } from './settings.svelte';

/** Progress and the autostart-once marker live in the user's settings blob so a
 *  tour resumes from any browser the user signs in from. */
export const settingsStorage: JourneyStorage = {
	get(key) {
		if (key === PROGRESS_KEY) return settings.onboarding.progress;
		const done = parseDoneKey(key);
		if (!done) return null;
		return settings.onboarding.seenVersion[done.id] === done.version ? '1' : null;
	},
	set(key, value) {
		if (key === PROGRESS_KEY) {
			settings.setOnboarding({ progress: value });
			return;
		}
		const done = parseDoneKey(key);
		if (!done) return;
		settings.setOnboarding({
			seenVersion: { ...settings.onboarding.seenVersion, [done.id]: done.version }
		});
	},
	remove(key) {
		if (key === PROGRESS_KEY) {
			settings.setOnboarding({ progress: null });
			return;
		}
		const done = parseDoneKey(key);
		if (!done) return;
		const { [done.id]: _, ...rest } = settings.onboarding.seenVersion;
		settings.setOnboarding({ seenVersion: rest });
	}
};

/** `journey:done:<id>@<version>` → its parts, or null for any other key. */
export function parseDoneKey(key: string): { id: string; version: number } | null {
	if (!key.startsWith(DONE_PREFIX)) return null;
	const at = key.lastIndexOf('@');
	if (at < DONE_PREFIX.length) return null;
	const version = Number(key.slice(at + 1));
	if (!Number.isInteger(version)) return null;
	return { id: key.slice(DONE_PREFIX.length, at), version };
}

/** Offered to a new user in this order; the rest of the book never reaches the app. */
export const ONBOARDING_JOURNEYS = [
	'enroll-machine',
	'accounts-pools',
	'spawn-session',
	'follow-session'
] as const;
export const REFERENCE_JOURNEYS = ['usage-overview', 'sessions-list', 'settings-tour'] as const;
export const PUBLIC_JOURNEYS: readonly string[] = [...ONBOARDING_JOURNEYS, ...REFERENCE_JOURNEYS];

/** A guide that cannot start until a probe holds, and the guide that makes it hold. */
export const GATES: Record<string, { probe: string; prerequisite: string }> = {
	'spawn-session': { probe: 'machines.online', prerequisite: 'enroll-machine' },
	'follow-session': { probe: 'sessions.live', prerequisite: 'spawn-session' }
};

/** Done is read from live state for the guides that produce something, so
 *  losing the last machine or account reopens the guide. */
export const DONE_PROBES: Record<string, string> = {
	'enroll-machine': 'machines.online',
	'accounts-pools': 'accounts',
	'spawn-session': 'sessions'
};

/** The toolbar collapses the conversation controls into panels at this width,
 *  which is what the mobile-only steps spotlight. */
export const MOBILE_QUERY = '(max-width: 959px)';

export const publicJourneys: IR[] = (journeys as IR[]).filter((j) => PUBLIC_JOURNEYS.includes(j.id));

/** Every `{name}` a journey's targets read from `params`. */
export function requiredParams(journey: Journey): string[] {
	const out = new Set<string>();
	const scan = (target: unknown) => {
		const path = typeof target === 'string' ? target : (target as { within?: string } | undefined)?.within;
		if (typeof path !== 'string') return;
		for (const m of path.matchAll(/\[\{([^{}]+)\}\]/g)) out.add(m[1]);
	};
	for (const step of journey.steps) {
		scan(step.target);
		for (const e of step.expect ?? []) {
			for (const v of Object.values(e)) scan(Array.isArray(v) ? v[0] : v);
		}
	}
	return [...out];
}

export type GuideParams = Record<string, string>;

/** The real-instance names the specs address through `{param}` keys. A name
 *  that does not exist stays absent, and the guide that needs it is refused. */
export async function guideParams(qc: QueryClient): Promise<GuideParams> {
	const out: GuideParams = { 'var.label': '', 'var.prompt': '' };
	const me = await qc.fetchQuery({ queryKey: ['me'], queryFn: endpoints.me, staleTime: 5 * 60_000 });
	if (me.user_name) out.me = me.user_name;
	const [accounts, pools, sessions] = await Promise.all([
		qc.fetchQuery({ queryKey: ['accounts'], queryFn: endpoints.accounts }),
		qc.fetchQuery({ queryKey: ['account-pools'], queryFn: endpoints.accountPools }),
		qc.fetchQuery({ queryKey: qk.sessions(false), queryFn: () => endpoints.sessions(false) })
	]);
	if (accounts[0]) out.account = accounts[0].name;
	if (pools[0]) out.pool = pools[0].name;
	const live = sessions.sessions.find(isLive);
	if (live) out.session = live.id;
	return out;
}

export function viewportVariant(
	matches: (query: string) => boolean = (q) => window.matchMedia(q).matches
): { viewport: 'mobile' | 'desktop' } {
	return { viewport: matches(MOBILE_QUERY) ? 'mobile' : 'desktop' };
}

export type StartOutcome =
	| { ok: true; result: RunResult }
	| { ok: false; reason: 'unknown' }
	| { ok: false; reason: 'gated'; prerequisite: string }
	| { ok: false; reason: 'missing'; params: string[] };

let host: { api: JourneyApi; qc: QueryClient; probes: Probes } | null = null;
let mounted: Promise<void> | null = null;

/** Mount the runtime once and register the public journeys. Waits for the
 *  server copy of the settings first, so a resumed tour reads the blob rather
 *  than the local cache. */
export function mountJourneys(qc: QueryClient): Promise<void> {
	mounted ??= (async () => {
		await settings.load();
		const probes = createProbes(qc);
		const api = mount({
			storage: settingsStorage,
			navigate: (route) => goto(route),
			probes
		});
		host = { api, qc, probes };
		await api.register(publicJourneys);
	})();
	return mounted;
}

/** Refuse a guide whose gate does not hold or whose real-instance names are
 *  missing; otherwise run it in guide mode and record the finish for the
 *  guides that have no probe to read it from. */
export async function startGuide(id: string): Promise<StartOutcome> {
	if (!host) throw new Error('journeys are not mounted');
	const ir = publicJourneys.find((j) => j.id === id);
	if (!ir) return { ok: false, reason: 'unknown' };
	const gate = GATES[id];
	if (gate && !(await host.probes[gate.probe]())) {
		return { ok: false, reason: 'gated', prerequisite: gate.prerequisite };
	}
	const params = await guideParams(host.qc);
	const missing = requiredParams(ir).filter((p) => !(p in params));
	if (missing.length) return { ok: false, reason: 'missing', params: missing };
	const result = await host.api.start(id, { mode: 'guide', params, variant: viewportVariant() });
	if (result.ok && !(id in DONE_PROBES)) {
		await settingsStorage.set(`${DONE_PREFIX}${id}@${ir.version}`, '1');
	}
	return { ok: true, result };
}

export async function guideDone(id: string): Promise<boolean> {
	const probe = DONE_PROBES[id];
	if (probe) {
		if (!host) return false;
		return Boolean(await host.probes[probe]());
	}
	const ir = publicJourneys.find((j) => j.id === id);
	return ir !== undefined && settings.onboarding.seenVersion[id] === ir.version;
}
