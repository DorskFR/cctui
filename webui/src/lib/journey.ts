import { browser } from '$app/environment';
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
	spotPresenter,
	type Strings,
	translator
} from '@dorsk/journey/runtime';
import { showConclusion } from './guideConclusion.svelte';
import journeys from './journeys.generated.json';
import { isLive } from './journeys/live';
import { createProbes, type Probes } from './journeys/probes';
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

/** The journeys that reach the app, in curriculum order — basics, setup, run,
 *  master. Which section a guide sits in, what unlocks it and what it is worth
 *  is the curriculum's to say; this list only decides what exists. The rest of
 *  the book never leaves the docs. */
export const PUBLIC_JOURNEYS: readonly string[] = [
	'welcome',
	'sessions-list',
	'accounts-pools',
	'enroll-machine',
	'spawn-session',
	'follow-session',
	'search-sessions',
	'usage-overview',
	'settings-tour'
];

/** Live instance state a guide needs before it can teach anything. This is a
 *  readiness signal the page states up front, never a refusal after the fact:
 *  what a guide is allowed to start is the curriculum's call, not a probe's. */
export const READINESS: Record<string, string> = {
	'spawn-session': 'machines.online',
	'follow-session': 'sessions.live'
};

const READINESS_HINTS: Record<string, () => string> = {
	'machines.online': () => m.journey_not_ready_machines_online(),
	'sessions.live': () => m.journey_not_ready_sessions_live()
};

export function readinessHint(id: string): string | undefined {
	const probe = READINESS[id];
	return probe === undefined ? undefined : READINESS_HINTS[probe]?.();
}

export async function guideReady(id: string): Promise<boolean> {
	const probe = READINESS[id];
	if (probe === undefined) return true;
	if (!host) return false;
	return Boolean(await host.probes[probe]());
}

export const GUIDES_ROUTE = '/settings/guides';

/** The guides whose lesson leaves something observable behind: a user who
 *  already has the result counts as done without taking the tour. */
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
	if (me.user_name) out['fixture.me'] = me.user_name;
	const [accounts, pools, sessions] = await Promise.all([
		qc.fetchQuery({ queryKey: ['accounts'], queryFn: endpoints.accounts }),
		qc.fetchQuery({ queryKey: ['account-pools'], queryFn: endpoints.accountPools }),
		qc.fetchQuery({ queryKey: qk.sessions(false), queryFn: () => endpoints.sessions(false) })
	]);
	if (accounts[0]) out.account = accounts[0].name;
	if (pools[0]) out.pool = pools[0].name;
	const live = sessions.sessions.find(isLive);
	if (live) out['fixture.session'] = live.id;
	return out;
}

export function viewportVariant(
	matches: (query: string) => boolean = (q) => window.matchMedia(q).matches
): { viewport: 'mobile' | 'desktop' } {
	return { viewport: matches(MOBILE_QUERY) ? 'mobile' : 'desktop' };
}

/** `ok` means the run reached its last step. A run the user exited is `aborted`
 *  and says nothing; a run that died on an expectation is `failed` and must. */
export type StartOutcome =
	| { ok: true; result: RunResult }
	| { ok: false; reason: 'unknown' }
	| { ok: false; reason: 'locked'; blockedBy: string[] }
	| { ok: false; reason: 'not-ready'; hint: string }
	| { ok: false; reason: 'missing'; params: string[] }
	| { ok: false; reason: 'aborted'; result: RunResult }
	| { ok: false; reason: 'failed'; result: RunResult };

export interface StartGuideOptions {
	/** Titles of the curriculum prerequisites that are not done yet. A non-empty
	 *  list refuses the guide; the caller has already said so on the page. */
	blockedBy?: readonly string[];
	/** Closing card copy. Omitted, the tour ends where its last step left off. */
	conclusion?: { title: string; xp: number };
}

const PARAM_NEEDS: Record<string, () => string> = {
	'fixture.me': () => m.journey_need_user(),
	account: () => m.journey_need_account(),
	pool: () => m.journey_need_pool(),
	'fixture.session': () => m.journey_need_session()
};

/** The one line a caller shows for a refused or broken run, or `undefined` when
 *  the user themselves ended it and has nothing to be told. */
export function startFailureMessage(outcome: StartOutcome): string | undefined {
	if (outcome.ok) return undefined;
	switch (outcome.reason) {
		case 'aborted':
			return undefined;
		case 'locked':
			return m.journey_locked({ guides: list(outcome.blockedBy) });
		case 'not-ready':
			return outcome.hint;
		case 'failed':
			return m.journey_failed();
		case 'missing': {
			const needs = outcome.params.flatMap((p) => {
				const need = PARAM_NEEDS[p];
				return need ? [need()] : [];
			});
			return needs.length ? m.journey_missing({ needs: list(needs) }) : m.journey_unavailable();
		}
		default:
			return m.journey_unavailable();
	}
}

function list(parts: readonly string[]): string {
	return parts.join(m.journey_list_separator());
}

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

/** The book driver marks its first navigation with `?journey=run`; later steps
 *  reload plain routes in the same tab, so the mark has to outlive the query. */
export const DRIVER_MARK = 'journey:driver';

export function driverRun(storage: Storage = sessionStorage, search = location.search): boolean {
	if (new URLSearchParams(search).get('journey') === 'run') storage.setItem(DRIVER_MARK, '1');
	return storage.getItem(DRIVER_MARK) === '1';
}

/** The slot forwarder parked in `app.html` waits on this; handing it the app's
 *  runtime is what releases the driver. */
export function resolveRuntime(api: JourneyApi, w: Window = window): void {
	(w as Window & { __journeyReady?: (api: JourneyApi) => void }).__journeyReady?.(api);
}

let mounted: Promise<void> | null = null;

/** Mount the runtime once and register the public journeys. Waits for the
 *  server copy of the settings first, so a resumed tour reads the blob rather
 *  than the local cache. */
export function mountJourneys(qc: QueryClient): Promise<void> {
	mounted ??= (async () => {
		const driver = browser && driverRun();
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
		// `mount` hands back whatever already holds `window.__journey`, so the
		// driver's forwarder has to go — but with no await before `mount` claims
		// the slot: a page.evaluate landing in that gap throws on undefined.
		if (driver) delete window.__journey;
		api = mount({
			storage: settingsStorage,
			navigate: (route) => goto(route),
			probes,
			translate,
			strings,
			presenter: (name) => {
				const t = translator(() => self().strings());
				if (name === 'guide') return deck;
				if (name === 'doc') return docPresenter(self().overlay, t);
				if (name === 'spot') return spotPresenter(self().overlay, t);
				return nonePresenter;
			}
		});
		host = { api, qc, probes };
		if (driver) resolveRuntime(api);
		// Registering arms `autostart`, which would draw the welcome deck over
		// whatever screen the driver is capturing; the driver hands it the IR.
		if (!driver) await api.register(publicJourneys);
		watchLocale(api);
	})();
	return mounted;
}

/** The runtime navigates on `step.route` only, so a journey that carries its
 *  opening route at the top level never leaves the page Replay was pressed on.
 *  A journey that anchors nothing anywhere — a carousel deck — is at home on
 *  whatever page it was started from, and moving it there is pure churn. */
export function entryRoute(ir: IR): string | undefined {
	const declared = ir.steps[0]?.route;
	if (declared) return declared;
	return ir.steps.some((step) => step.target !== undefined) ? ir.route : undefined;
}

export async function startGuide(id: string, opts: StartGuideOptions = {}): Promise<StartOutcome> {
	if (!host) throw new Error('journeys are not mounted');
	const ir = publicJourneys.find((j) => j.id === id);
	if (!ir) return { ok: false, reason: 'unknown' };
	if (opts.blockedBy?.length) return { ok: false, reason: 'locked', blockedBy: [...opts.blockedBy] };
	if (!(await guideReady(id))) {
		return { ok: false, reason: 'not-ready', hint: readinessHint(id) ?? m.journey_unavailable() };
	}
	const params = await guideParams(host.qc);
	const missing = requiredParams(ir).filter((p) => !(p in params));
	if (missing.length) return { ok: false, reason: 'missing', params: missing };
	lastParams = params;
	const entry = entryRoute(ir);
	if (entry && location.pathname !== entry) await goto(entry);
	const result = await host.api.start(id, { mode: 'guide', params, variant: viewportVariant() });
	if (!result.ok) {
		return { ok: false, reason: result.aborted ? 'aborted' : 'failed', result };
	}
	await settingsStorage.set(`${DONE_PREFIX}${id}@${ir.version}`, '1');
	if (opts.conclusion) await concludeGuide(opts.conclusion);
	return { ok: true, result };
}

async function concludeGuide(conclusion: { title: string; xp: number }): Promise<void> {
	await showConclusion(conclusion);
	if (location.pathname !== GUIDES_ROUTE) await goto(GUIDES_ROUTE);
}

/** A guide with a `DONE_PROBES` entry produces something observable, so the
 *  state counts as done even for a user who never took the tour — and losing it
 *  does not undo a tour they did take. */
export async function guideDone(id: string): Promise<boolean> {
	const ir = publicJourneys.find((j) => j.id === id);
	if (!ir) return false;
	if (settings.onboarding.seenVersion[id] === ir.version) return true;
	const probe = DONE_PROBES[id];
	if (probe === undefined || !host) return false;
	return Boolean(await host.probes[probe]());
}

export async function guidesDone(ids: readonly string[]): Promise<Record<string, boolean>> {
	const done = await Promise.all(ids.map((id) => guideDone(id)));
	return Object.fromEntries(ids.map((id, i) => [id, done[i]]));
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
