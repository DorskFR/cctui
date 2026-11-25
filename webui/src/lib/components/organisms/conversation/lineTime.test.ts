import { describe, expect, it } from 'vitest';
import { shortTime } from './lineTime';

describe('line time', () => {
	it('reads as 24-hour hours and minutes, with no seconds or day period', () => {
		const at = new Date(2026, 10, 26, 21, 48, 57).getTime();
		expect(shortTime(at)).toMatch(/^21.48$/);
		expect(shortTime(new Date(2026, 10, 26, 9, 5, 1).getTime())).toMatch(/^09.05$/);
	});
});
