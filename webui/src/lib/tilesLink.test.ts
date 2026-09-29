import { describe, expect, it } from 'vitest';
import { tilesHref } from './tilesLink';
import { navItems } from './navItems';

describe('tilesHref', () => {
	it('comma-joins the set and drops blanks and duplicates', () => {
		expect(tilesHref(['a', 'b'])).toBe('/tiles?s=a,b');
		expect(tilesHref(['a', 'a', '', 'b'])).toBe('/tiles?s=a,b');
	});

	it('is the bare route with nothing to open', () => {
		expect(tilesHref([])).toBe('/tiles');
	});
});

describe('tiles nav item', () => {
	it('sits right after Sessions', () => {
		const hrefs = navItems().map((i) => i.href);
		expect(hrefs).toContain('/tiles');
		expect(hrefs.indexOf('/tiles')).toBe(hrefs.indexOf('/sessions') + 1);
	});
});
