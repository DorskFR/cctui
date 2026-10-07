import { describe, expect, it } from 'vitest';
import {
	AUTO_LIMIT_RESET_DEFAULT,
	AUTO_LIMIT_RESET_KEY,
	readAutoLimitReset,
	writeAutoLimitReset
} from './auto-limit-reset.logic';

describe('readAutoLimitReset', () => {
	it('defaults to off when the key is absent or malformed', () => {
		expect(readAutoLimitReset(null)).toEqual(AUTO_LIMIT_RESET_DEFAULT);
		expect(readAutoLimitReset({})).toEqual(AUTO_LIMIT_RESET_DEFAULT);
		expect(readAutoLimitReset({ [AUTO_LIMIT_RESET_KEY]: 'yes' })).toEqual(AUTO_LIMIT_RESET_DEFAULT);
		expect(readAutoLimitReset({ [AUTO_LIMIT_RESET_KEY]: [1] })).toEqual(AUTO_LIMIT_RESET_DEFAULT);
	});

	it('reads the stored knobs and falls back per field', () => {
		const got = readAutoLimitReset({
			[AUTO_LIMIT_RESET_KEY]: { enabled: true, used_pct: 75, weekly_max_pct: -1 }
		});
		expect(got).toEqual({ enabled: true, used_pct: 75, expires_within_hours: 24, weekly_max_pct: 80 });
		expect(readAutoLimitReset({ [AUTO_LIMIT_RESET_KEY]: { enabled: 'true' } }).enabled).toBe(false);
	});
});

describe('writeAutoLimitReset', () => {
	it('keeps the other provider settings and writes the policy beside them', () => {
		const out = writeAutoLimitReset(
			{ session_affinity: true },
			{ enabled: true, used_pct: 95, expires_within_hours: 12, weekly_max_pct: 50 }
		);
		expect(out).toEqual({
			session_affinity: true,
			[AUTO_LIMIT_RESET_KEY]: { enabled: true, used_pct: 95, expires_within_hours: 12, weekly_max_pct: 50 }
		});
	});

	it('drops the key when the policy is off at its defaults', () => {
		const out = writeAutoLimitReset(
			{ session_affinity: true, [AUTO_LIMIT_RESET_KEY]: { enabled: true } },
			{ ...AUTO_LIMIT_RESET_DEFAULT }
		);
		expect(out).toEqual({ session_affinity: true });
	});

	it('round-trips through read', () => {
		const policy = { enabled: true, used_pct: 80, expires_within_hours: 6, weekly_max_pct: 70 };
		expect(readAutoLimitReset(writeAutoLimitReset({}, policy))).toEqual(policy);
	});

	it('repairs a blank number input to its default', () => {
		const out = writeAutoLimitReset({}, {
			enabled: true,
			used_pct: '' as unknown as number,
			expires_within_hours: Number.NaN,
			weekly_max_pct: 70
		});
		expect(out[AUTO_LIMIT_RESET_KEY]).toEqual({
			enabled: true,
			used_pct: 90,
			expires_within_hours: 24,
			weekly_max_pct: 70
		});
	});
});
