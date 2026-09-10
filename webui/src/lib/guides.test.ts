import type { Journey } from '@dorsk/journey';
import { beforeEach, describe, expect, it } from 'vitest';
import { auth } from './auth.svelte';
import { clearGuide, guideEntries, guideStatus, progressJourneyId, resetGuides } from './guides';
import { settings } from './settings.svelte';
import type { OnboardingSettings } from './settings.svelte';

const catalogue = [
	{ id: 'a', version: 2, title: 'Alpha', description: 'First', steps: [] },
	{ id: 'b', title: 'Beta', steps: [] }
] as unknown as Journey[];

function onboarding(patch: Partial<OnboardingSettings> = {}): OnboardingSettings {
	return { seenVersion: {}, progress: null, ...patch };
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
});

describe('clearGuide / resetGuides', () => {
	beforeEach(() => {
		auth.isAuthed = false;
		localStorage.clear();
		settings.setOnboarding({ seenVersion: {}, progress: null });
	});

	it('drops only the named guide and keeps another journey resuming', () => {
		const progress = JSON.stringify({ id: 'b' });
		settings.setOnboarding({ seenVersion: { a: 2, b: 1 }, progress });
		clearGuide('a');
		expect(settings.onboarding).toEqual({ seenVersion: { b: 1 }, progress });
	});

	it('drops the resume record when it belongs to the cleared guide', () => {
		settings.setOnboarding({ seenVersion: { a: 2 }, progress: JSON.stringify({ id: 'a' }) });
		clearGuide('a');
		expect(settings.onboarding).toEqual({ seenVersion: {}, progress: null });
	});

	it('resets every marker so the first-run deck comes back', () => {
		settings.setOnboarding({ seenVersion: { a: 2, b: 1 }, progress: JSON.stringify({ id: 'b' }) });
		resetGuides();
		expect(settings.onboarding).toEqual({ seenVersion: {}, progress: null });
	});
});
