import { describe, expect, it } from 'vitest';
import { fmtAge, fmtAgo } from '$lib/diagnoseSilence';
import { parityFixture } from './fixtures';

type Case = { ms: number; unit: string; value: number; bare: string; ago: string };

const fx = parityFixture<{ cases: Case[] }>('age');

describe('age parity', () => {
	for (const c of fx.cases) {
		it(`${c.ms}ms reads as ${c.bare}`, () => {
			expect(fmtAge(c.ms)).toBe(c.bare);
			expect(fmtAgo(c.ms)).toBe(c.ago);
		});
	}

	it('an undated age says so rather than counting from zero', () => {
		expect(fmtAge(null)).toBe(fmtAgo(null));
		expect(fmtAge(null)).not.toContain('0');
	});
});
