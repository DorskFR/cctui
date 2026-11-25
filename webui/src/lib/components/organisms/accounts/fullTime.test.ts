import { describe, expect, it } from 'vitest';
import { fullTime } from './fullTime';

describe('fullTime', () => {
	it('spells out a valid instant and stays silent on missing or bad input', () => {
		expect(fullTime('2026-07-10T08:00:00Z')).toMatch(/2026/);
		expect(fullTime(null)).toBeUndefined();
		expect(fullTime('not a date')).toBeUndefined();
	});
});
