import { describe, expect, it } from 'vitest';
import { buildSessionSearchSchema, splitQuery } from './searchSchema';
import { dirLabel, isScoped, sinceDay, toggleScope } from './searchScope';

const schema = buildSessionSearchSchema(async () => []);
const now = Date.parse('2026-11-05T10:00:00Z');

describe('toggleScope', () => {
	it('appends a machine clause the server reads and removes it on the second press', () => {
		const on = toggleScope('deploy fix', schema, { kind: 'machine', value: 'box-1' }, now);
		expect(on).toBe('deploy fix machine=box-1');
		expect(splitQuery(on, schema).serverQuery).toBe('deploy fix machine=box-1');
		expect(isScoped(on, schema, { kind: 'machine', value: 'box-1' }, now)).toBe(true);
		expect(toggleScope(on, schema, { kind: 'machine', value: 'box-1' }, now)).toBe('deploy fix');
	});

	it('quotes values with spaces so they round-trip', () => {
		const q = toggleScope('', schema, { kind: 'machine', value: 'my box' }, now);
		expect(q).toBe('machine="my box"');
		expect(isScoped(q, schema, { kind: 'machine', value: 'my box' }, now)).toBe(true);
	});

	it('scopes to a directory with a contains clause', () => {
		const q = toggleScope('bug', schema, { kind: 'dir', value: '/work/cctui' }, now);
		expect(q).toBe('bug dir:/work/cctui');
		expect(isScoped(q, schema, { kind: 'dir', value: '/work/cctui' }, now)).toBe(true);
	});

	it('replaces an earlier clause of the same kind', () => {
		const q = toggleScope('machine=a bug', schema, { kind: 'machine', value: 'b' }, now);
		expect(q).toBe('bug machine=b');
		const t = toggleScope('bug created>=2020-01-01', schema, { kind: 'time', value: '7d' }, now);
		expect(t).toBe('bug created>=2026-10-29');
	});

	it('turns a time scope into a client-side created bound', () => {
		const q = toggleScope('bug', schema, { kind: 'time', value: '24h' }, now);
		expect(q).toBe('bug created>=2026-11-04');
		const { serverQuery, clientFilters } = splitQuery(q, schema);
		expect(serverQuery).toBe('bug');
		expect(clientFilters.map((f) => [f.field, f.op, f.values])).toEqual([['created', 'gte', ['2026-11-04']]]);
		expect(isScoped(q, schema, { kind: 'time', value: '24h' }, now)).toBe(true);
		expect(isScoped(q, schema, { kind: 'time', value: '7d' }, now)).toBe(false);
	});

	it('keeps scopes of different kinds side by side', () => {
		let q = toggleScope('bug', schema, { kind: 'machine', value: 'm' }, now);
		q = toggleScope(q, schema, { kind: 'dir', value: '/w' }, now);
		q = toggleScope(q, schema, { kind: 'time', value: '30d' }, now);
		expect(q).toBe('bug machine=m dir:/w created>=2026-10-06');
		expect(toggleScope(q, schema, { kind: 'dir', value: '/w' }, now)).toBe('bug machine=m created>=2026-10-06');
	});
});

describe('sinceDay', () => {
	it('counts whole UTC days back', () => {
		expect(sinceDay('24h', now)).toBe('2026-11-04');
		expect(sinceDay('7d', now)).toBe('2026-10-29');
		expect(sinceDay('30d', now)).toBe('2026-10-06');
	});
});

describe('dirLabel', () => {
	it('shows the last path segment', () => {
		expect(dirLabel('/home/u/work/cctui/')).toBe('cctui');
		expect(dirLabel('/')).toBe('/');
	});
});
