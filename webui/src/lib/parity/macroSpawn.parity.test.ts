import { describe, expect, it } from 'vitest';
import type { MacroSpec } from '$lib/settings.svelte';
import { macroProblems, spawnBodyFor } from '$lib/components/organisms/macros.logic';
import { parityFixture } from './fixtures';

type Fixture = {
	spawnBodyFor: { macro: MacroSpec; out: Record<string, unknown> }[];
	macroProblems: { macro: MacroSpec; out: string[] }[];
};

const fx = parityFixture<Fixture>('macroSpawn');

/** The fixture states only the fields both clients send; the web body carries
 *  the same set, so a missing key on either side is a difference. */
describe('macro spawn parity fixtures', () => {
	it('spawnBodyFor', () => {
		for (const c of fx.spawnBodyFor)
			expect(spawnBodyFor(c.macro) as unknown as Record<string, unknown>, JSON.stringify(c)).toEqual(
				c.out
			);
	});
	it('macroProblems', () => {
		for (const c of fx.macroProblems)
			expect(macroProblems(c.macro), JSON.stringify(c)).toEqual(c.out);
	});
});
