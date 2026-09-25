// @vitest-environment happy-dom
import { beforeEach, describe, expect, it } from 'vitest';
import { auth } from './auth.svelte';
import { mergeOnboarding, ratchetStepProgress, settings } from './settings.svelte';

function record(id: string, index: number, steps = 5, version = 1): string {
	return JSON.stringify({ id, index, version, ir: { steps: Array.from({ length: steps }) } });
}

describe('mergeOnboarding', () => {
	it('reads a blob written before step progress existed', () => {
		expect(mergeOnboarding({ seenVersion: { a: 2 }, progress: null })).toEqual({
			seenVersion: { a: 2 },
			progress: null,
			stepProgress: {},
			probeOptOut: []
		});
	});

	it('keeps the probe opt-out list, dropping duplicates and non-strings', () => {
		expect(mergeOnboarding({ probeOptOut: ['a', 'a', 7, 'b'] }).probeOptOut).toEqual(['a', 'b']);
		expect(mergeOnboarding({ probeOptOut: 'nope' }).probeOptOut).toEqual([]);
	});

	it('keeps a well-formed step record', () => {
		const stepProgress = { a: { index: 2, total: 5, version: 3 } };
		expect(mergeOnboarding({ stepProgress }).stepProgress).toEqual(stepProgress);
	});

	it('drops a step record with no usable index', () => {
		const stepProgress = { a: { total: 5, version: 1 }, b: { index: 'two' } };
		expect(mergeOnboarding({ stepProgress }).stepProgress).toEqual({});
	});

	it('defaults a missing total and version rather than dropping the record', () => {
		expect(mergeOnboarding({ stepProgress: { a: { index: 4 } } }).stepProgress).toEqual({
			a: { index: 4, total: 0, version: 1 }
		});
	});
});

describe('ratchetStepProgress', () => {
	it('records the step and the run length out of a resume record', () => {
		expect(ratchetStepProgress({}, record('a', 2))).toEqual({
			a: { index: 2, total: 5, version: 1 }
		});
	});

	it('moves forward but never backwards within a version', () => {
		const at3 = ratchetStepProgress({}, record('a', 3));
		expect(ratchetStepProgress(at3, record('a', 1))).toBe(at3);
		expect(ratchetStepProgress(at3, record('a', 4)).a.index).toBe(4);
	});

	it('restarts the count when the guide version moves', () => {
		const old = ratchetStepProgress({}, record('a', 3, 5, 1));
		expect(ratchetStepProgress(old, record('a', 0, 6, 2)).a).toEqual({
			index: 0,
			total: 6,
			version: 2
		});
	});

	it('leaves other guides alone', () => {
		const both = ratchetStepProgress(ratchetStepProgress({}, record('a', 1)), record('b', 2));
		expect(Object.keys(both).sort()).toEqual(['a', 'b']);
	});

	it('is a no-op for a cleared or unreadable record', () => {
		const prev = { a: { index: 1, total: 5, version: 1 } };
		expect(ratchetStepProgress(prev, null)).toBe(prev);
		expect(ratchetStepProgress(prev, 'not json')).toBe(prev);
		expect(ratchetStepProgress(prev, JSON.stringify({ index: 9 }))).toBe(prev);
	});
});

describe('setOnboarding', () => {
	beforeEach(() => {
		auth.isAuthed = false;
		localStorage.clear();
		settings.setOnboarding({ seenVersion: {}, progress: null, stepProgress: {} });
	});

	it('ratchets the furthest step every time the runtime writes progress', () => {
		settings.setOnboarding({ progress: record('a', 2) });
		settings.setOnboarding({ progress: record('a', 4) });
		expect(settings.onboarding.stepProgress).toEqual({ a: { index: 4, total: 5, version: 1 } });
	});

	it('keeps the furthest step when the run ends and the record is dropped', () => {
		settings.setOnboarding({ progress: record('a', 3) });
		settings.setOnboarding({ progress: null });
		expect(settings.onboarding.progress).toBeNull();
		expect(settings.onboarding.stepProgress).toEqual({ a: { index: 3, total: 5, version: 1 } });
	});

	it('lets an explicit step patch win over the ratchet', () => {
		settings.setOnboarding({ progress: record('a', 3) });
		settings.setOnboarding({ progress: record('a', 4), stepProgress: {} });
		expect(settings.onboarding.stepProgress).toEqual({});
	});
});
