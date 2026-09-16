import { readFileSync } from 'node:fs';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { QueryClient } from '@tanstack/svelte-query';
import { DONE_PREFIX, PROGRESS_KEY } from '@dorsk/journey/runtime';
import type { IR, Journey } from '@dorsk/journey';
import type { JourneyApi } from '@dorsk/journey/runtime';
import { resolveText } from '@dorsk/journey/runtime';
import journeys from './journeys.generated.json';
import { locale } from './locale.svelte';
import {
	driverRun,
	entryRoute,
	forgetProgress,
	guideDone,
	guideParams,
	guidesDone,
	GUIDES_ROUTE,
	isDeck,
	MOBILE_QUERY,
	parseDoneKey,
	publicJourneys,
	READINESS,
	readinessHint,
	requiredParams,
	resolveRuntime,
	settingsStorage,
	type StartOutcome,
	startFailureMessage,
	strings,
	translate,
	viewportVariant
} from './journey';
import { auth } from './auth.svelte';
import { mergeDefaults, settings } from './settings.svelte';

const api = vi.hoisted(() => ({
	me: vi.fn(),
	accounts: vi.fn(),
	accountPools: vi.fn(),
	sessions: vi.fn()
}));
vi.mock('$lib/queries/endpoints', () => ({ endpoints: api }));

const KEY = 'cctui_settings';

function blob() {
	return mergeDefaults(JSON.parse(localStorage.getItem(KEY) ?? 'null')).onboarding;
}

beforeEach(() => {
	auth.isAuthed = false;
	localStorage.clear();
	settings.setOnboarding({ seenVersion: {}, progress: null, stepProgress: {} });
});

describe('parseDoneKey', () => {
	it('splits the runtime done marker into id and version', () => {
		expect(parseDoneKey(`${DONE_PREFIX}sessions-list@3`)).toEqual({ id: 'sessions-list', version: 3 });
		expect(parseDoneKey(`${DONE_PREFIX}a@b@2`)).toEqual({ id: 'a@b', version: 2 });
	});
	it('rejects anything else', () => {
		expect(parseDoneKey(PROGRESS_KEY)).toBeNull();
		expect(parseDoneKey(`${DONE_PREFIX}sessions-list`)).toBeNull();
		expect(parseDoneKey(`${DONE_PREFIX}sessions-list@x`)).toBeNull();
	});
});

describe('settingsStorage', () => {
	it('keeps progress in data.onboarding.progress', async () => {
		const record = JSON.stringify({ id: 'sessions-list', version: 1, index: 1 });
		await settingsStorage.set(PROGRESS_KEY, record);
		expect(blob().progress).toBe(record);
		expect(await settingsStorage.get(PROGRESS_KEY)).toBe(record);
		await settingsStorage.remove(PROGRESS_KEY);
		expect(blob().progress).toBeNull();
		expect(await settingsStorage.get(PROGRESS_KEY)).toBeNull();
	});

	it('keeps the done marker as seenVersion[id], matched on the exact version', async () => {
		const key = `${DONE_PREFIX}sessions-list@2`;
		expect(await settingsStorage.get(key)).toBeNull();
		await settingsStorage.set(key, '1');
		expect(blob().seenVersion).toEqual({ 'sessions-list': 2 });
		expect(await settingsStorage.get(key)).toBe('1');
		expect(await settingsStorage.get(`${DONE_PREFIX}sessions-list@3`)).toBeNull();
		await settingsStorage.remove(key);
		expect(blob().seenVersion).toEqual({});
	});

	it('ignores keys it does not own', async () => {
		await settingsStorage.set('journey:other', 'x');
		expect(await settingsStorage.get('journey:other')).toBeNull();
		expect(blob()).toEqual({ seenVersion: {}, progress: null, stepProgress: {}, probeOptOut: [] });
	});
});

describe('requiredParams', () => {
	it('lists every {param} key a journey addresses', () => {
		expect(
			requiredParams({
				id: 'x',
				steps: [
					{ id: 'a', target: 'user[{me}]', expect: [{ visible: 'tab[keys]' }] },
					{ id: 'b', target: { role: 'tab', within: 'account[{account}]' } },
					{ id: 'c', expect: [{ count: ['pool[{pool}]/member', { min: 1 }] }, { probe: 'accounts' }] }
				]
			})
		).toEqual(['me', 'account', 'pool']);
	});
	it('ignores literal keys', () => {
		expect(requiredParams({ id: 'x', steps: [{ id: 'a', target: 'tab[machines]' }] })).toEqual([]);
	});
});

describe('viewportVariant', () => {
	it('maps the toolbar breakpoint to the mobile variant', () => {
		expect(viewportVariant((q) => q === MOBILE_QUERY)).toEqual({ viewport: 'mobile' });
		expect(viewportVariant(() => false)).toEqual({ viewport: 'desktop' });
	});
});

describe('guideParams', () => {
	beforeEach(() => {
		for (const fn of Object.values(api)) fn.mockReset();
		api.me.mockResolvedValue({ role: 'admin', user_id: 'u1', user_name: 'root' });
		api.accounts.mockResolvedValue([]);
		api.accountPools.mockResolvedValue([]);
		api.sessions.mockResolvedValue({ sessions: [] });
	});
	const qc = () => new QueryClient({ defaultOptions: { queries: { retry: false } } });

	it('leaves absent names out so the guide that needs them is refused', async () => {
		expect(await guideParams(qc())).toEqual({ 'var.label': '', 'var.prompt': '', 'fixture.me': 'root' });
	});

	it('names the first account, pool and live session', async () => {
		api.accounts.mockResolvedValue([{ name: 'main' }, { name: 'other' }]);
		api.accountPools.mockResolvedValue([{ name: 'prod' }]);
		api.sessions.mockResolvedValue({
			sessions: [
				{ id: 'd', status: 'draft', liveness: 'dead' },
				{ id: 'l', status: 'active', liveness: 'active' }
			]
		});
		expect(await guideParams(qc())).toMatchObject({ 'fixture.me': 'root', account: 'main', pool: 'prod', 'fixture.session': 'l' });
	});
});

describe('refusing and failing a guide', () => {
	const failed = { ok: false, completed: 1, failures: [{ stepId: 'where', error: 'timeout' }] };

	it('names the prerequisite guides the caller resolved, never a journey id', () => {
		const message = startFailureMessage({
			ok: false,
			reason: 'locked',
			blockedBy: ['Bring a machine into the fleet']
		});
		expect(message).toContain('Bring a machine into the fleet');
		expect(message).not.toContain('enroll-machine');
	});

	it('says which instance state a guide is waiting for', () => {
		expect(READINESS['spawn-session']).toBe('machines.online');
		const hint = readinessHint('spawn-session')!;
		expect(hint).toBeTruthy();
		expect(startFailureMessage({ ok: false, reason: 'not-ready', hint })).toBe(hint);
		expect(readinessHint('usage-overview')).toBeUndefined();
	});

	it('tells the user a run died rather than letting it vanish', () => {
		const outcome: StartOutcome = { ok: false, reason: 'failed', result: failed };
		expect(startFailureMessage(outcome)).toBeTruthy();
	});

	it('says nothing when the user exited the tour themselves', () => {
		expect(
			startFailureMessage({ ok: false, reason: 'aborted', result: { ...failed, aborted: true } })
		).toBeUndefined();
	});

	it('names what a missing param stands for, and falls back when it names nothing', () => {
		expect(startFailureMessage({ ok: false, reason: 'missing', params: ['account'] })).toContain(
			'account'
		);
		expect(
			startFailureMessage({ ok: false, reason: 'missing', params: ['var.nothing'] })
		).toBe(startFailureMessage({ ok: false, reason: 'unknown' }));
	});

	it('has nothing to say about a run that finished', () => {
		expect(startFailureMessage({ ok: true, result: { ok: true, completed: 3, failures: [] } })).toBeUndefined();
	});
});

describe('guideDone', () => {
	const ir = () => publicJourneys[0]!;

	it('is done once the marker matches the version that ran', async () => {
		expect(await guideDone(ir().id)).toBe(false);
		await settingsStorage.set(`${DONE_PREFIX}${ir().id}@${ir().version}`, '1');
		expect(await guideDone(ir().id)).toBe(true);
	});

	it('is not done when the guide moved on, and never for an id it does not know', async () => {
		await settingsStorage.set(`${DONE_PREFIX}${ir().id}@${ir().version + 1}`, '1');
		expect(await guideDone(ir().id)).toBe(false);
		expect(await guideDone('no-such-guide')).toBe(false);
	});

	it('answers for a whole list at once, which is what the guides page asks', async () => {
		await settingsStorage.set(`${DONE_PREFIX}${ir().id}@${ir().version}`, '1');
		const done = await guidesDone([ir().id, 'no-such-guide']);
		expect(done).toEqual({ [ir().id]: true, 'no-such-guide': false });
	});
});

describe('a guide starts only from a click', () => {
	it('declares no autostart on any public journey', () => {
		for (const j of publicJourneys) expect(j.autostart, j.id).toBeUndefined();
	});

	it('drops the progress the runtime would replay on register', () => {
		const storage = { removeItem: vi.fn() } as unknown as Storage;
		forgetProgress(storage);
		expect(storage.removeItem).toHaveBeenCalledWith(PROGRESS_KEY);
	});
});

describe('the guides page is where a tour hands back', () => {
	it('points at the page the guides are launched from', () => {
		expect(GUIDES_ROUTE).toBe('/settings/guides');
	});

	it('opens an anchored guide on a route, whether the step or the journey carries it', () => {
		for (const j of publicJourneys) {
			if (!j.steps.some((s) => s.target !== undefined)) continue;
			expect(entryRoute(j), j.id).toBeTruthy();
		}
	});

	it('sends a target-less journey nowhere, so a replay does not leave the guides page', () => {
		const deck = { id: 'deck', route: '/', steps: [{ id: 'card' }] } as unknown as IR;
		expect(isDeck(deck)).toBe(true);
		expect(deck.route, 'the deck still declares a route of its own').toBeTruthy();
		expect(entryRoute(deck)).toBeUndefined();
	});

	it('anchors every guide the curriculum ships', () => {
		expect(publicJourneys.filter(isDeck).map((j) => j.id)).toEqual([]);
	});

	it('tells a deck apart from a tour that anchors even one step', () => {
		const anchored = publicJourneys.filter((j) => !isDeck(j));
		expect(anchored.length).toBeGreaterThan(0);
		for (const j of anchored) expect(j.steps.some((s) => s.target !== undefined), j.id).toBe(true);
	});
});

describe('journey copy and chrome follow the active locale', () => {
	const ir = journeys as unknown as Journey[];
	const texts = (loc: string) =>
		ir.flatMap((journey) => {
			const step = (t: unknown) => resolveText(t as never, translate, loc);
			return [
				step(journey.title),
				step(journey.description),
				...(journey.steps ?? []).flatMap((s) => [step(s.say?.title), step(s.say?.body)])
			];
		});

	it('renders a message id that has no message as the id, never a blank card', () => {
		expect(translate('journey_next')).toBe('Next');
		expect(translate('no_such_message_at_all')).toBeUndefined();
		expect(resolveText({ $msg: 'no_such_message_at_all' }, translate, 'en')).toBe(
			'no_such_message_at_all'
		);
	});

	it('hands the runtime its own placeholders back rather than filling them in', () => {
		const s = strings();
		expect(s.step).toContain('{i}');
		expect(s.step).toContain('{n}');
		expect(s.goToPageBody).toContain('{route}');
	});

	it('localises the library chrome', () => {
		locale.set('en');
		expect(strings().next).toBe('Next');
		locale.set('fr');
		expect(strings().next).toBe('Suivant');
		expect(strings().step).toContain('{i}');
		locale.set('en');
	});

	it('leaves no card blank in either locale', () => {
		for (const loc of ['en', 'fr'])
			for (const t of texts(loc)) expect(t === undefined || t.length > 0).toBe(true);
	});

	it('says something different in French', () => {
		const en = texts('en');
		const fr = texts('fr');
		expect(fr.filter((t, i) => t !== en[i]).length).toBeGreaterThan(0);
	});
});

describe('book driver slot', () => {
	const shim = readFileSync('src/app.html', 'utf8').match(
		/<script>([\s\S]*?)<\/script>/
	);

	function runShim(search: string): void {
		history.replaceState(null, '', `/${search}`);
		new Function(shim?.[1] ?? '')();
	}

	beforeEach(() => {
		sessionStorage.clear();
		delete window.__journey;
		delete (window as Window & { __journeyReady?: unknown }).__journeyReady;
	});

	it('remembers the driver mark across the plain routes the driver reloads', () => {
		expect(driverRun(sessionStorage, '')).toBe(false);
		expect(driverRun(sessionStorage, '?journey=run')).toBe(true);
		expect(driverRun(sessionStorage, '')).toBe(true);
	});

	it('parks the slot in the document head, before the app can boot', () => {
		expect(shim?.[1]).toContain('__journeyReady');
	});

	it('leaves the slot alone outside a driver run', () => {
		runShim('');
		expect(window.__journey).toBeUndefined();
	});

	it('holds the slot so the driver cannot mount a probe-less runtime', () => {
		runShim('?journey=run');
		expect(window.__journey).toBeDefined();
		expect(sessionStorage.getItem('journey:driver')).toBe('1');
	});

	it('keeps the slot filled for the driver right up to the real mount', () => {
		const source = readFileSync('src/lib/journey.ts', 'utf8');
		const handover = source.slice(
			source.indexOf('delete window.__journey'),
			source.indexOf('api = mount(')
		);
		expect(handover).not.toBe('');
		expect(handover).not.toMatch(/\bawait\b/);
	});

	it('forwards the calls it parked to the app runtime once that mounts', async () => {
		runShim('?journey=run');
		const parked = window.__journey?.driver.step();
		const real = { driver: { step: () => Promise.resolve({ done: true }) } } as unknown as JourneyApi;
		resolveRuntime(real, window);
		await expect(parked).resolves.toEqual({ done: true });
	});
});
