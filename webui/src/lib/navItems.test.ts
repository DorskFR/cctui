import { describe, expect, it } from 'vitest';

import { isNavActive, navItems } from './navItems';

describe('navItems', () => {
	it('lists the route tabs with sessions second and settings last', () => {
		const hrefs = navItems().map((i) => i.href);
		expect(hrefs[0]).toBe('/');
		expect(hrefs[1]).toBe('/sessions');
		expect(hrefs.at(-1)).toBe('/settings');
	});

	it('ends the built-in run at bookmarks: admin screens live under Settings', () => {
		const hrefs = navItems().map((i) => i.href);
		expect(hrefs).toEqual(['/', '/sessions', '/bookmarks', '/settings']);
	});

	it('keeps access, accounts and dispatchers out of the nav', () => {
		const hrefs = navItems().map((i) => i.href);
		for (const gone of ['/access', '/accounts', '/dispatchers', '/users']) {
			expect(hrefs).not.toContain(gone);
		}
	});

	it('inserts plugin pages before settings', () => {
		const hrefs = navItems({
			pages: [{ href: '/apps/ghreview', label: 'Review', iconName: 'pull-request' }]
		}).map((i) => i.href);
		expect(hrefs.indexOf('/apps/ghreview')).toBe(hrefs.indexOf('/settings') - 1);
	});

	it('names a kit icon for every built-in entry', () => {
		for (const item of navItems()) expect(item.iconName).toBeTruthy();
	});

	it('marks the root only on an exact match and the others by prefix', () => {
		expect(isNavActive('/', '/')).toBe(true);
		expect(isNavActive('/', '/sessions')).toBe(false);
		expect(isNavActive('/sessions', '/sessions/abc')).toBe(true);
		expect(isNavActive('/settings', '/sessions')).toBe(false);
	});
});
