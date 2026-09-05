import { describe, expect, it } from 'vitest';
import { isNavActive, navItems } from './navItems';

describe('navItems', () => {
	it('lists the route tabs with sessions second and settings last', () => {
		const hrefs = navItems().map((i) => i.href);
		expect(hrefs[0]).toBe('/');
		expect(hrefs[1]).toBe('/sessions');
		expect(hrefs.at(-1)).toBe('/settings');
	});

	it('marks the root only on an exact match and the others by prefix', () => {
		expect(isNavActive('/', '/')).toBe(true);
		expect(isNavActive('/', '/sessions')).toBe(false);
		expect(isNavActive('/sessions', '/sessions/abc')).toBe(true);
		expect(isNavActive('/users', '/sessions')).toBe(false);
	});
});
