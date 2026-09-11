import { beforeEach, describe, expect, it, vi } from 'vitest';

const { ghreviewUrl } = vi.hoisted(() => ({ ghreviewUrl: vi.fn<() => string | null>() }));
vi.mock('./config', () => ({ ghreviewUrl }));

import { isNavActive, navItems } from './navItems';

beforeEach(() => {
	ghreviewUrl.mockReturnValue(null);
});

describe('navItems github gate', () => {
	const hasGithub = (gates?: Parameters<typeof navItems>[0]) =>
		navItems(gates).some((i) => i.href === '/github');

	it('hides github when ghreview is undeployed and there is no connector', () => {
		expect(hasGithub({ hasGithubConnector: false })).toBe(false);
	});

	it('hides github when ghreview is undeployed even if a connector is claimed', () => {
		expect(hasGithub({ hasGithubConnector: true })).toBe(false);
	});

	it('hides github when ghreview is deployed but no connector exists', () => {
		ghreviewUrl.mockReturnValue('https://ghreview.example');
		expect(hasGithub({ hasGithubConnector: false })).toBe(false);
	});

	it('shows github only when ghreview is deployed and a connector exists', () => {
		ghreviewUrl.mockReturnValue('https://ghreview.example');
		expect(hasGithub({ hasGithubConnector: true })).toBe(true);
		expect(navItems({ hasGithubConnector: true }).find((i) => i.href === '/github')?.href).toBe(
			'/github'
		);
	});

	it('defaults to hidden when no gate is passed', () => {
		ghreviewUrl.mockReturnValue('https://ghreview.example');
		expect(hasGithub()).toBe(false);
	});
});

describe('navItems', () => {
	it('lists the route tabs with sessions second and settings last', () => {
		const hrefs = navItems().map((i) => i.href);
		expect(hrefs[0]).toBe('/');
		expect(hrefs[1]).toBe('/sessions');
		expect(hrefs.at(-1)).toBe('/settings');
	});

	it('places bookmarks between sessions and access', () => {
		const hrefs = navItems().map((i) => i.href);
		expect(hrefs.indexOf('/bookmarks')).toBe(hrefs.indexOf('/sessions') + 1);
		expect(hrefs.indexOf('/access')).toBe(hrefs.indexOf('/bookmarks') + 1);
	});

	it('exposes access as the merged users/keys/machines/tokens route', () => {
		const access = navItems().find((i) => i.href === '/access');
		expect(access?.label).toBe('Access');
		expect(navItems().some((i) => i.href === '/users')).toBe(false);
	});

	it('marks the root only on an exact match and the others by prefix', () => {
		expect(isNavActive('/', '/')).toBe(true);
		expect(isNavActive('/', '/sessions')).toBe(false);
		expect(isNavActive('/sessions', '/sessions/abc')).toBe(true);
		expect(isNavActive('/access', '/sessions')).toBe(false);
	});
});
