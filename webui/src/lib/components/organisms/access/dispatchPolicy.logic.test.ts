import { describe, expect, it } from 'vitest';
import {
	blankPack,
	formFromRules,
	incompletePacks,
	invalidLimits,
	parseLimit,
	parseList,
	rulesFromForm
} from './dispatchPolicy.logic';

describe('parseList', () => {
	it('splits on commas, spaces and newlines and dedups', () => {
		expect(parseList(' kube, local\nkube  docker,')).toEqual(['kube', 'local', 'docker']);
	});
	it('is empty for blank text', () => {
		expect(parseList('  \n ')).toEqual([]);
	});
});

describe('parseLimit', () => {
	it('keeps positive integers only', () => {
		expect(parseLimit('30')).toBe(30);
		expect(parseLimit(' 7 ')).toBe(7);
		for (const bad of ['', '0', '-1', '1.5', 'abc']) expect(parseLimit(bad)).toBeUndefined();
	});
});

describe('form round trip', () => {
	it('starts blank without a policy', () => {
		const form = formFromRules(null);
		expect(form.dispatchers).toBe('');
		expect(form.packs).toEqual([]);
		expect(rulesFromForm(form)).toEqual({
			dispatchers: [],
			profiles: [],
			adapters: [],
			models: [],
			context_packs: [],
			max_timeout_minutes: undefined,
			max_concurrent: undefined,
			daily_cap: undefined
		});
	});

	it('maps every rule both ways and drops blank packs and unpinned fields', () => {
		const rules = {
			dispatchers: ['cctui-kube'],
			profiles: ['tonbo'],
			adapters: ['claude-code'],
			models: ['sonnet', 'haiku'],
			context_packs: [{ name: 'p', url: 'https://git/p.git', ref: 'main' }],
			max_timeout_minutes: 30,
			max_concurrent: 2,
			daily_cap: 50
		};
		const form = formFromRules(rules);
		expect(form.models).toBe('sonnet, haiku');
		expect(form.packs[0]).toEqual({ name: 'p', url: 'https://git/p.git', ref: 'main', subdir: '' });
		form.packs.push(blankPack());
		expect(rulesFromForm(form)).toEqual(rules);
	});

	it('flags a pack with a name but no url', () => {
		const form = formFromRules(null);
		form.packs.push({ ...blankPack(), name: 'p' });
		expect(incompletePacks(form)).toBe(true);
		form.packs[0].url = 'https://x';
		expect(incompletePacks(form)).toBe(false);
	});
});

describe('invalidLimits', () => {
	const form = (limits: Partial<Record<'maxTimeout' | 'maxConcurrent' | 'dailyCap', string>>) => ({
		...formFromRules(null),
		...limits
	});
	it('accepts positive integers', () => {
		expect(invalidLimits(form({ maxTimeout: '30', maxConcurrent: ' 2 ', dailyCap: '100' }))).toEqual([]);
	});
	it('treats blank as no limit', () => {
		expect(invalidLimits(form({ maxTimeout: '', maxConcurrent: '   ', dailyCap: '' }))).toEqual([]);
	});
	it.each(['10m', '1.5', '-3', '0', 'abc'])('flags %s in every limit field', (bad) => {
		expect(invalidLimits(form({ maxTimeout: bad }))).toEqual(['maxTimeout']);
		expect(invalidLimits(form({ maxConcurrent: bad }))).toEqual(['maxConcurrent']);
		expect(invalidLimits(form({ dailyCap: bad }))).toEqual(['dailyCap']);
	});
	it('lists every offending field', () => {
		expect(invalidLimits(form({ maxTimeout: '0', maxConcurrent: '4', dailyCap: '1.5' }))).toEqual([
			'maxTimeout',
			'dailyCap'
		]);
	});
});
