import { describe, expect, it } from 'vitest';
import { turnIdFrom } from '$lib/turnid';
import { parityFixture } from './fixtures';

type Fixture = { turnIdFrom: { ts: number; random: number[]; out: string }[] };

const fx = parityFixture<Fixture>('turnid');

describe('turnid parity fixtures', () => {
	it('turnIdFrom', () => {
		for (const c of fx.turnIdFrom) expect(turnIdFrom(c.ts, c.random), JSON.stringify(c)).toBe(c.out);
	});
});
