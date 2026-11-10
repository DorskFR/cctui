// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Feature } from '@bindings/Feature';
import { setCapabilitiesForTest } from './features.svelte';
import { guideEntries } from './guides';
import { GUIDE_FEATURES, offeredJourneys, publicJourneys, STEP_FEATURES } from './journey';

vi.mock('$lib/queries/endpoints', () => ({ endpoints: {} }));

const SWITCHABLE: Feature[] = [
	'machine_spawn',
	'transcript_search',
	'account_sharing',
	'automation',
	'organising'
];

afterEach(() => setCapabilitiesForTest(null));

const stepIds = (id: string, has: (f: Feature) => boolean) =>
	offeredJourneys(has)
		.find((j) => j.id === id)
		?.steps.map((s) => s.id);

describe('offeredJourneys', () => {
	it('is the public book, step for step, with every feature on', () => {
		expect(offeredJourneys(() => true)).toEqual(publicJourneys);
	});

	it('drops the machine and search guides on a core server', () => {
		const ids = offeredJourneys(() => false).map((j) => j.id);
		expect(ids).not.toContain('enroll-machine');
		expect(ids).not.toContain('spawn-session');
		expect(ids).not.toContain('search-sessions');
		expect(ids).toEqual(expect.arrayContaining(['welcome', 'sessions-list', 'accounts-pools', 'follow-session']));
	});

	it('drops the steps that point at a surface an off feature owns', () => {
		const off = () => false;
		expect(stepIds('sessions-list', off)).not.toContain('search');
		expect(stepIds('sessions-list', off)).not.toContain('clear');
		expect(stepIds('accounts-pools', off)).not.toContain('pools');
		expect(stepIds('usage-overview', off)).not.toContain('capacity');
		expect(stepIds('settings-tour', off)).not.toContain('sessions');
		expect(stepIds('settings-tour', off)?.[0]).toBe('theme');
	});

	it('drops only the steps of the feature that is off', () => {
		const noPools = (f: Feature) => f !== 'account_sharing';
		expect(stepIds('accounts-pools', noPools)).not.toContain('pools');
		expect(stepIds('sessions-list', noPools)).toContain('search');
	});

	it('names only guides and steps the book ships, so a rename cannot slip past', () => {
		const byId = new Map(publicJourneys.map((j) => [j.id, j]));
		for (const id of Object.keys(GUIDE_FEATURES)) expect(byId.has(id), id).toBe(true);
		for (const [id, steps] of Object.entries(STEP_FEATURES)) {
			const ir = byId.get(id);
			expect(ir, id).toBeDefined();
			const known = new Set(ir?.steps.map((s) => s.id));
			for (const step of Object.keys(steps)) expect(known.has(step), `${id}/${step}`).toBe(true);
		}
	});
});

describe('the guides list on a core server', () => {
	it('offers no guide whose subject is off', () => {
		setCapabilitiesForTest({ features: [], switchable: SWITCHABLE });
		const ids = guideEntries().map((e) => e.id);
		expect(ids).not.toContain('enroll-machine');
		expect(ids).not.toContain('spawn-session');
		expect(ids).not.toContain('search-sessions');
		expect(ids).toContain('accounts-pools');
	});

	it('offers every public guide with every feature on', () => {
		setCapabilitiesForTest({ features: SWITCHABLE, switchable: SWITCHABLE });
		expect(guideEntries().map((e) => e.id)).toEqual(publicJourneys.map((j) => j.id));
	});
});
