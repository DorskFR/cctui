import { describe, expect, it } from 'vitest';
import { fullTime, openFamilies, openRedirectTargets } from './redirect.logic';

const chip = (family: string) => ({ id: `r-${family}`, family, targetName: 'x', until: null });

describe('openFamilies', () => {
	it('keeps families this account runs that no rule redirects yet', () => {
		const t = { id: 't', name: 'T', families: ['claude', 'openai', 'qwen'] };
		expect(openFamilies(t, ['claude', 'openai'], [chip('openai')])).toEqual(['claude']);
	});
});

describe('openRedirectTargets', () => {
	it('drops targets with nothing left to take over', () => {
		const a = { id: 'a', name: 'A', families: ['claude'] };
		const b = { id: 'b', name: 'B', families: ['openai'] };
		expect(openRedirectTargets([a, b], ['claude', 'openai'], [chip('claude')])).toEqual([b]);
	});
});

describe('fullTime', () => {
	it('is undefined for absent or unparseable input', () => {
		expect(fullTime(null)).toBeUndefined();
		expect(fullTime('nope')).toBeUndefined();
	});
	it('formats a parseable instant', () => {
		expect(fullTime('2026-07-10T12:00:00Z')).toBe(new Date('2026-07-10T12:00:00Z').toLocaleString());
	});
});
