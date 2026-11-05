import { describe, expect, it } from 'vitest';
import { exposureView } from './dispatchExposure';

const all = { untrusted_input: true, sensitive_data: true, outward: true };

describe('exposureView', () => {
	it('shows nothing for a run whose dispatcher reported no exposure', () => {
		expect(exposureView(null)).toBeNull();
		expect(exposureView(undefined)).toBeNull();
	});

	it('lists the three legs in order with which are held', () => {
		const view = exposureView({ untrusted_input: true, sensitive_data: false, outward: true });
		expect(view?.flags.map((f) => [f.key, f.label, f.on])).toEqual([
			['untrusted_input', 'untrusted input', true],
			['sensitive_data', 'sensitive data', false],
			['outward', 'outward', true]
		]);
		expect(view?.gate).toBeNull();
		expect(view?.ungated).toBe(false);
	});

	it('names each human gate', () => {
		expect(exposureView({ ...all, human_gate: 'no_push' })?.gate).toBe('no push');
		expect(exposureView({ ...all, human_gate: 'draft_pr_only' })?.gate).toBe('draft PR only');
		expect(exposureView({ ...all, human_gate: 'outward_held_for_approval' })?.gate).toBe(
			'outward held for approval'
		);
		expect(exposureView({ ...all, human_gate: 'draft_pr_only' })?.ungated).toBe(false);
	});

	it('flags all three legs held with no gate', () => {
		expect(exposureView(all)?.ungated).toBe(true);
	});
});
