import { describe, expect, it } from 'vitest';
import {
	ALL_SCOPES,
	filterByName,
	keyIcon,
	scopeCells,
	visibleRows
} from '$lib/components/organisms/access/access.logic';
import { parityFixture } from './fixtures';

type Fixture = {
	allScopes: string[];
	scopeCells: { granted: string[]; out: boolean[] }[];
	visibleOrder: { revoked: boolean[]; show_revoked: boolean; out: number[] }[];
	filterByName: { names: string[]; query: string; out: number[] }[];
	keyIcon: { kind: string; out: string }[];
};

const fx = parityFixture<Fixture>('admin');

describe('access.logic parity fixtures', () => {
	it('allScopes', () => {
		expect([...ALL_SCOPES]).toEqual(fx.allScopes);
	});
	it('scopeCells', () => {
		for (const c of fx.scopeCells)
			expect(
				scopeCells(c.granted).map((cell) => cell.granted),
				JSON.stringify(c)
			).toEqual(c.out);
	});
	it('visibleOrder', () => {
		for (const c of fx.visibleOrder) {
			const rows = c.revoked.map((r, i) => ({ i, revoked_at: r ? '2026-01-01T00:00:00Z' : null }));
			expect(
				visibleRows(rows, c.show_revoked).map((r) => r.i),
				JSON.stringify(c)
			).toEqual(c.out);
		}
	});
	it('filterByName', () => {
		for (const c of fx.filterByName) {
			const rows = c.names.map((name, i) => ({ name, i }));
			expect(
				filterByName(rows, c.query).map((r) => r.i),
				JSON.stringify(c)
			).toEqual(c.out);
		}
	});
	it('keyIcon', () => {
		for (const c of fx.keyIcon) expect(keyIcon(c.kind), JSON.stringify(c)).toBe(c.out);
	});
});
