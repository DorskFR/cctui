import type { Journey } from '@dorsk/journey';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { auth } from './auth.svelte';
import {
	buildCurriculum,
	clearGuide,
	guideOptions,
	CURRICULUM,
	GUIDE_SECTIONS,
	guideEntries,
	guideStatus,
	progressJourneyId,
	replayGuide,
	resetGuides
} from './guides';
import type { GuideSectionId } from './guides';
import { DONE_PROBES, GUIDES_ROUTE, startGuide } from './journey';
import { settings } from './settings.svelte';
import type { OnboardingSettings } from './settings.svelte';

vi.mock('./journey', async (original) => ({
	...(await original<typeof import('./journey')>()),
	startGuide: vi.fn()
}));

const started = vi.mocked(startGuide);

const catalogue = [
	{ id: 'a', version: 2, title: 'Alpha', description: 'First', steps: [] },
	{ id: 'b', title: 'Beta', steps: [] }
] as unknown as Journey[];

/** One journey per curriculum id, so the whole table is available. */
const fullCatalogue = CURRICULUM.map(
	(c) => ({ id: c.id, title: c.id.toUpperCase(), steps: [] }) as unknown as Journey
);

function onboarding(patch: Partial<OnboardingSettings> = {}): OnboardingSettings {
	return { seenVersion: {}, progress: null, stepProgress: {}, probeOptOut: [], ...patch };
}

/** Every id of every section up to and including `section`. */
function through(section: GuideSectionId): Record<string, number> {
	const limit = GUIDE_SECTIONS.indexOf(section);
	const seen: Record<string, number> = {};
	for (const c of CURRICULUM) {
		if (GUIDE_SECTIONS.indexOf(c.section) <= limit) seen[c.id] = 1;
	}
	return seen;
}

describe('guideEntries', () => {
	it('normalizes titles, descriptions and versions', () => {
		expect(guideEntries(catalogue)).toEqual([
			{ id: 'a', version: 2, title: 'Alpha', description: 'First' },
			{ id: 'b', version: 1, title: 'Beta', description: '' }
		]);
	});

	it('exposes every bundled journey', () => {
		expect(guideEntries().length).toBeGreaterThan(0);
		expect(guideEntries().every((e) => e.id && e.title)).toBe(true);
	});
});

describe('progressJourneyId', () => {
	it('reads the id out of a resume record', () => {
		expect(progressJourneyId(JSON.stringify({ id: 'a', index: 3 }))).toBe('a');
	});

	it('is null for absent or unreadable records', () => {
		expect(progressJourneyId(null)).toBeNull();
		expect(progressJourneyId('not json')).toBeNull();
		expect(progressJourneyId(JSON.stringify({ index: 3 }))).toBeNull();
	});
});

describe('guideStatus', () => {
	const [alpha, beta] = guideEntries(catalogue);

	it('is not-started with nothing stored', () => {
		expect(guideStatus(alpha, onboarding())).toBe('not-started');
	});

	it('is done when the stored version matches', () => {
		expect(guideStatus(alpha, onboarding({ seenVersion: { a: 2 } }))).toBe('done');
	});

	it('is not-started again when the journey moved past the seen version', () => {
		expect(guideStatus(alpha, onboarding({ seenVersion: { a: 1 } }))).toBe('not-started');
	});

	it('is in-progress for the journey the resume record belongs to', () => {
		const state = onboarding({ seenVersion: { a: 2 }, progress: JSON.stringify({ id: 'a' }) });
		expect(guideStatus(alpha, state)).toBe('in-progress');
		expect(guideStatus(beta, state)).toBe('not-started');
	});

	it('is done when live state says so even with no stored marker', () => {
		expect(guideStatus(alpha, onboarding(), true)).toBe('done');
	});

	it('is in-progress for a tour abandoned part way through', () => {
		const state = onboarding({ stepProgress: { a: { index: 2, total: 5, version: 2 } } });
		expect(guideStatus(alpha, state)).toBe('in-progress');
	});

	it('ignores a step record left by an older version of the guide', () => {
		const state = onboarding({ stepProgress: { a: { index: 2, total: 5, version: 1 } } });
		expect(guideStatus(alpha, state)).toBe('not-started');
	});

	it('tolerates a blob written before step progress existed', () => {
		const legacy = { seenVersion: {}, progress: null } as unknown as OnboardingSettings;
		expect(guideStatus(alpha, legacy)).toBe('not-started');
	});
});

describe('buildCurriculum', () => {
	const entries = guideEntries(fullCatalogue);

	it('orders the guides by section then position', () => {
		const view = buildCurriculum(entries, onboarding());
		expect(view.sections.map((s) => s.id)).toEqual(['basics', 'setup', 'run', 'master']);
		expect(view.sections.flatMap((s) => s.guides.map((g) => g.id))).toEqual([
			'welcome',
			'sessions-list',
			'accounts-pools',
			'enroll-machine',
			'spawn-session',
			'follow-session',
			'search-sessions',
			'usage-overview',
			'settings-tour'
		]);
	});

	it('totals 180 XP across nine guides', () => {
		const view = buildCurriculum(entries, onboarding());
		expect(view.totalXp).toBe(180);
		expect(view.totalCount).toBe(9);
		expect(view.earnedXp).toBe(0);
		expect(view.doneCount).toBe(0);
	});

	it('awards the XP of every completed guide', () => {
		const view = buildCurriculum(entries, onboarding({ seenVersion: through('basics') }));
		expect(view.earnedXp).toBe(25);
		expect(view.doneCount).toBe(2);
	});

	it('leaves the first section open and locks every later one', () => {
		const view = buildCurriculum(entries, onboarding());
		expect(view.sections.map((s) => s.locked)).toEqual([false, true, true, true]);
		expect(view.sections[0].guides.every((g) => !g.locked)).toBe(true);
		expect(view.sections[1].guides.every((g) => g.locked)).toBe(true);
	});

	it('unlocks a section once every preceding guide is done', () => {
		const view = buildCurriculum(entries, onboarding({ seenVersion: through('basics') }));
		expect(view.sections[1].locked).toBe(false);
		expect(view.sections[2].locked).toBe(true);
	});

	it('names the unfinished guides by title rather than by id', () => {
		const view = buildCurriculum(entries, onboarding());
		expect(view.sections[1].lockedBy).toEqual(['WELCOME', 'SESSIONS-LIST']);
		expect(view.sections[1].guides[0].lockedBy).toEqual(['WELCOME', 'SESSIONS-LIST']);
	});

	it('keeps a guide locked by an unfinished prerequisite inside its own section', () => {
		const view = buildCurriculum(entries, onboarding({ seenVersion: through('setup') }));
		const run = view.sections[2];
		expect(run.locked).toBe(false);
		expect(run.guides[0].locked).toBe(false);
		expect(run.guides[1].id).toBe('follow-session');
		expect(run.guides[1].locked).toBe(true);
		expect(run.guides[1].lockedBy).toEqual(['SPAWN-SESSION']);
	});

	it('never locks a guide that is already done', () => {
		const view = buildCurriculum(entries, onboarding({ seenVersion: { 'settings-tour': 1 } }));
		const tour = view.sections[3].guides.find((g) => g.id === 'settings-tour');
		expect(tour?.locked).toBe(false);
		expect(tour?.status).toBe('done');
	});

	it('drops a curriculum id this build ships no journey for', () => {
		const partial = guideEntries(fullCatalogue.filter((j) => j.id !== 'welcome'));
		const view = buildCurriculum(partial, onboarding());
		expect(view.totalCount).toBe(8);
		expect(view.totalXp).toBe(170);
		expect(view.sections[0].guides.map((g) => g.id)).toEqual(['sessions-list']);
		expect(view.sections[1].lockedBy).toEqual(['SESSIONS-LIST']);
	});

	it('surfaces the furthest step of a half-finished tour', () => {
		const view = buildCurriculum(
			entries,
			onboarding({ stepProgress: { welcome: { index: 3, total: 8, version: 1 } } })
		);
		const welcome = view.sections[0].guides[0];
		expect(welcome.status).toBe('in-progress');
		expect(welcome.step).toEqual({ index: 3, total: 8 });
	});

	it('reports no step for a finished tour', () => {
		const view = buildCurriculum(
			entries,
			onboarding({
				seenVersion: { welcome: 1 },
				stepProgress: { welcome: { index: 3, total: 8, version: 1 } }
			})
		);
		expect(view.sections[0].guides[0].step).toBeNull();
	});

	it('hands the runtime an empty lock for a guide the curriculum allows', () => {
		const [guide] = buildCurriculum(entries, onboarding()).sections[0].guides;
		expect(guideOptions(guide)).toEqual({
			blockedBy: [],
			conclusion: { title: 'WELCOME', xp: 10 },
			returnTo: GUIDES_ROUTE
		});
	});

	it('reads live-state completion for the guides that write no marker', () => {
		const view = buildCurriculum(entries, onboarding(), { welcome: true });
		expect(view.sections[0].guides[0].status).toBe('done');
		expect(view.earnedXp).toBe(10);
	});
});

describe('clearGuide / resetGuides', () => {
	beforeEach(() => {
		auth.isAuthed = false;
		localStorage.clear();
		settings.setOnboarding({ seenVersion: {}, progress: null, stepProgress: {} });
	});

	it('drops only the named guide and keeps another journey resuming', () => {
		const progress = JSON.stringify({ id: 'b' });
		settings.setOnboarding({ seenVersion: { a: 2, b: 1 }, progress, stepProgress: {} });
		clearGuide('a');
		expect(settings.onboarding).toEqual({ seenVersion: { b: 1 }, progress, stepProgress: {}, probeOptOut: [] });
	});

	it('drops the resume record when it belongs to the cleared guide', () => {
		settings.setOnboarding({
			seenVersion: { a: 2 },
			progress: JSON.stringify({ id: 'a' }),
			stepProgress: {}
		});
		clearGuide('a');
		expect(settings.onboarding).toEqual({
			seenVersion: {},
			progress: null,
			stepProgress: {},
			probeOptOut: []
		});
	});

	it('drops the furthest-step record of the cleared guide only', () => {
		settings.setOnboarding({
			stepProgress: { a: { index: 2, total: 5, version: 1 }, b: { index: 1, total: 3, version: 1 } }
		});
		clearGuide('a');
		expect(settings.onboarding.stepProgress).toEqual({ b: { index: 1, total: 3, version: 1 } });
	});

	it('resets every marker so the first-run deck comes back', () => {
		settings.setOnboarding({
			seenVersion: { a: 2, b: 1 },
			progress: JSON.stringify({ id: 'b' }),
			stepProgress: { a: { index: 2, total: 5, version: 1 } }
		});
		resetGuides();
		expect(settings.onboarding.seenVersion).toEqual({});
		expect(settings.onboarding.progress).toBeNull();
		expect(settings.onboarding.stepProgress).toEqual({});
	});

	/** The probe-backed guides read their completion from live instance state,
	 *  which a reset cannot undo; without the opt-out they report done again at
	 *  once and the curriculum bar never moves. */
	it('suppresses probe-derived completion so the progress bar actually drops', () => {
		resetGuides();
		expect(settings.onboarding.probeOptOut.sort()).toEqual(Object.keys(DONE_PROBES).sort());
	});
});

describe('replayGuide', () => {
	beforeEach(() => {
		auth.isAuthed = false;
		localStorage.clear();
		settings.setOnboarding({ seenVersion: {}, progress: null, stepProgress: {} });
		started.mockReset();
	});

	it('clears the done marker before the guide runs', async () => {
		settings.setOnboarding({ seenVersion: { a: 2 } });
		started.mockImplementation(async () => {
			expect(settings.onboarding.seenVersion.a).toBeUndefined();
			return { ok: true, result: { ok: true } } as never;
		});
		await replayGuide('a');
		expect(started).toHaveBeenCalledWith('a', {});
	});

	it('puts the done marker back when the start is refused', async () => {
		settings.setOnboarding({ seenVersion: { a: 2 } });
		started.mockResolvedValue({ ok: false, reason: 'locked', blockedBy: ['Beta'] });
		await replayGuide('a');
		expect(settings.onboarding.seenVersion).toEqual({ a: 2 });
	});

	it('keeps the done marker when a replay is abandoned part way', async () => {
		settings.setOnboarding({ seenVersion: { a: 2 } });
		started.mockResolvedValue({ ok: false, reason: 'aborted', result: { ok: false } as never });
		await replayGuide('a');
		expect(settings.onboarding.seenVersion).toEqual({ a: 2 });
	});

	it('forwards the curriculum lock and the closing card to the runtime', async () => {
		started.mockResolvedValue({ ok: false, reason: 'unknown' });
		const [guide] = buildCurriculum(guideEntries(fullCatalogue), onboarding()).sections[1].guides;
		await replayGuide(guide.id, guideOptions(guide));
		expect(started).toHaveBeenCalledWith(guide.id, {
			blockedBy: ['WELCOME', 'SESSIONS-LIST'],
			conclusion: { title: guide.title, xp: guide.xp },
			returnTo: GUIDES_ROUTE
		});
	});

	it('leaves a never-completed guide unmarked when the start is refused', async () => {
		started.mockResolvedValue({ ok: false, reason: 'unknown' });
		await replayGuide('a');
		expect(settings.onboarding.seenVersion).toEqual({});
	});
});
