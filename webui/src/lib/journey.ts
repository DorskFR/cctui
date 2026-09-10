import { goto } from '$app/navigation';
import type { QueryClient } from '@tanstack/svelte-query';
import type { IR, Journey } from '@dorsk/journey';
import {
	docPresenter,
	DONE_PREFIX,
	guidePresenter,
	nonePresenter,
	type JourneyApi,
	type JourneyStorage,
	mount,
	type Presenter,
	PROGRESS_KEY,
	type RunResult,
	type Strings,
	translator
} from '@dorsk/journey/runtime';
import journeys from './journeys.generated.json';
import { createProbes, isLive, type Probes } from './journeys/probes';
import { m } from './paraglide/messages';
import { endpoints } from './queries/endpoints';
import { qk } from './queries/keys';
import { settings } from './settings.svelte';
import { type DeckCard, deckPresenter } from './welcomeDeck.svelte';

/** A spec may name a message id instead of carrying the copy. An id with no
 *  message resolves to the id itself upstream, which is ugly but readable —
 *  better than a blank card. */
export function translate(id: string): string | undefined {
	const message = (m as Record<string, unknown>)[id];
	return typeof message === 'function' ? (message() as string) : undefined;
}

/** The runtime interpolates `{i}`/`{n}`/`{route}` itself, so paraglide has to
 *  hand back the braces rather than fill them in. */
export function strings(): Partial<Strings> {
	return {
		next: m.journey_next(),
		exit: m.journey_exit(),
		step: m.journey_step({ i: '{i}', n: '{n}' }),
		goToPage: m.journey_go_to_page(),
		goToPageBody: m.journey_go_to_page_body({ route: '{route}' }),
		goToPageAction: m.journey_go_to_page_action(),
		press: m.journey_press()
	};
}

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
let lastParams: GuideParams = {};

/** The running journey's steps as deck cards. The presenter is handed one step at
 *  a time, but a carousel needs the whole deck up front. */
function deckCards(api: JourneyApi): DeckCard[] {
	const engine = api.engine();
	if (!engine) return [];
	return engine.ir.steps.map((step) => ({
		title: engine.text(step.say?.title) ?? '',
		body: engine.text(step.say?.body) ?? ''
	}));
}

function markSeen(api: JourneyApi): void {
	const ir = api.engine()?.ir;
	if (!ir) return;
	settingsStorage.set(`${DONE_PREFIX}${ir.id}@${ir.version}`, '1');
}

let mounted: Promise<void> | null = null;

/** Mount the runtime once and register the public journeys. Waits for the
 *  server copy of the settings first, so a resumed tour reads the blob rather
 *  than the local cache. */
export function mountJourneys(qc: QueryClient): Promise<void> {
	mounted ??= (async () => {
		await settings.load();
		const probes = createProbes(qc);
		let api: JourneyApi | null = null;
		let overlay: Presenter | null = null;
		const self = () => {
			if (!api) throw new Error('journey runtime is not mounted yet');
			return api;
		};
		const fallback = () =>
			(overlay ??= guidePresenter(self().overlay, translator(() => self().strings())));
		const deck = deckPresenter({
			cards: () => deckCards(self()),
			markSeen: () => markSeen(self()),
			fallback
		});
		api = mount({
			storage: settingsStorage,
			navigate: (route) => goto(route),
			probes,
			translate,
			strings,
			presenter: (name) =>
				name === 'guide' ? deck : name === 'doc' ? docPresenter(self().overlay) : nonePresenter
		});
		host = { api, qc, probes };
		await api.register(publicJourneys);
		watchLocale(api);
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
	lastParams = params;
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

/** Copy and chrome are resolved when a card is drawn, so a language switch only
 *  reaches an open guide by drawing it again at the same step. `lang` is the
 *  locale the runtime itself reads, so it is the signal worth following. The
 *  re-draw has to carry the params and variant the run started with, or the
 *  real-instance names its targets address vanish mid-guide. */
function watchLocale(api: JourneyApi): void {
	const root = document.documentElement;
	let lang = root.lang;
	new MutationObserver(() => {
		if (root.lang === lang) return;
		lang = root.lang;
		const current = api.current();
		if (!current) return;
		void api.start(current.id, {
			mode: 'guide',
			from: current.index,
			params: lastParams,
			variant: viewportVariant()
		});
	}).observe(root, { attributeFilter: ['lang'] });
}
