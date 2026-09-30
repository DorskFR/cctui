import { describe, expect, it } from 'vitest';
import { load } from './+page';
import { SESSIONS_TILES_HREF } from './redirect';

describe('the retired /tiles route', () => {
	it('forwards to Sessions in tiles mode', () => {
		let caught: unknown;
		try {
			load();
		} catch (e) {
			caught = e;
		}
		expect(caught).toMatchObject({ status: 307, location: SESSIONS_TILES_HREF });
	});

	it('points at the sessions page, not at a page of its own', () => {
		expect(SESSIONS_TILES_HREF).toBe('/sessions?view=tiles');
	});
});
