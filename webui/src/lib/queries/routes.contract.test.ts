import { describe, expect, it } from 'vitest';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { API_PREFIX, ROUTES, path, route, url } from '@bindings/routes';

describe('the generated route table', () => {
	it('is not empty and has unique ids', () => {
		expect(ROUTES.length).toBeGreaterThan(100);
		expect(new Set(ROUTES.map((r) => r.id)).size).toBe(ROUTES.length);
	});

	it('has unique method+path pairs', () => {
		const pairs = ROUTES.map((r) => `${r.method} ${r.path}`);
		expect(new Set(pairs).size).toBe(pairs.length);
	});

	it('builds paths and urls from the template', () => {
		expect(path('get_me')).toBe('/me');
		expect(url('get_me')).toBe(`${API_PREFIX}/me`);
		expect(path('patch_profiles_by_id', { id: 'abc' })).toBe('/profiles/abc');
		expect(url('patch_profiles_by_id', { id: 'abc' })).toBe(`${API_PREFIX}/profiles/abc`);
	});

	it('encodes substituted params', () => {
		expect(path('patch_profiles_by_id', { id: 'a/b' })).toBe('/profiles/a%2Fb');
	});

	it('leaves an unsupplied placeholder in place rather than dropping it', () => {
		expect(path('patch_profiles_by_id')).toBe('/profiles/{id}');
	});

	it('refuses an unknown id', () => {
		// @ts-expect-error — the point of RouteId is that this does not typecheck.
		expect(() => route('no_such_route')).toThrow();
	});

	it('carries every request/response type name as an exported binding', () => {
		const dir = join(process.cwd(), 'src', 'lib', 'bindings');
		const known = new Set(readdirSync(dir).map((f) => f.replace(/\.ts$/, '')));
		const missing: string[] = [];
		for (const r of ROUTES) {
			for (const t of [r.request, r.response]) {
				if (t && !known.has(t.replace(/\[\]$/, ''))) missing.push(`${r.id}: ${t}`);
			}
		}
		expect(missing).toEqual([]);
	});
});

describe('endpoint helpers use the route table', () => {
	it('every converted endpoint names a real route', () => {
		const src = readFileSync(join(process.cwd(), 'src', 'lib', 'queries', 'endpoints.ts'), 'utf8');
		const ids = [...src.matchAll(/\bpath\("([a-z0-9_]+)"/g)].map((m) => m[1]);
		expect(ids.length).toBeGreaterThan(0);
		const known = new Set(ROUTES.map((r) => r.id));
		expect(ids.filter((id) => !known.has(id))).toEqual([]);
	});
});
