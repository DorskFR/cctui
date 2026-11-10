import { afterEach, describe, expect, it } from 'vitest';

import type { Feature } from '@bindings/Feature';
import { setCapabilitiesForTest } from './features.svelte';
import { isNavActive, navItems } from './navItems';

const SWITCHABLE: Feature[] = [
	'machine_spawn', 'files', 'uploads', 'drafts', 'profiles', 'terminal', 'transcript_search',
	'child_sessions', 'context', 'plugins', 'auto_approve', 'scheduling', 'automation', 'rooms',
	'voice', 'previews', 'federation', 'device_login', 'passkeys', 'self_update', 'account_sharing',
	'observability', 'library', 'organising'
];
const PLUGIN_PAGE = [{ href: '/apps/ghreview', label: 'Review', iconName: 'pull-request' as const }];

describe('navItems', () => {
	it('lists the route tabs with sessions second and settings last', () => {
		const hrefs = navItems().map((i) => i.href);
		expect(hrefs[0]).toBe('/');
		expect(hrefs[1]).toBe('/sessions');
		expect(hrefs.at(-1)).toBe('/settings');
	});

	it('ends the built-in run at accounts: the rest lives under Settings', () => {
		const hrefs = navItems().map((i) => i.href);
		expect(hrefs).toEqual(['/', '/sessions', '/bookmarks', '/accounts', '/settings']);
	});

	it('keeps access, dispatchers and events out of the nav', () => {
		const hrefs = navItems().map((i) => i.href);
		for (const gone of ['/access', '/dispatchers', '/events', '/users']) {
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

describe('navItems by announced features', () => {
	afterEach(() => setCapabilitiesForTest(null));

	it('drops bookmarks and plugin pages on a core server', () => {
		setCapabilitiesForTest({ features: [], switchable: SWITCHABLE });
		const hrefs = navItems({ pages: PLUGIN_PAGE }).map((i) => i.href);
		expect(hrefs).toEqual(['/', '/sessions', '/accounts', '/settings']);
	});

	it('lists plugin pages with every feature on', () => {
		setCapabilitiesForTest({ features: SWITCHABLE, switchable: SWITCHABLE });
		const hrefs = navItems({ pages: PLUGIN_PAGE }).map((i) => i.href);
		expect(hrefs).toEqual(['/', '/sessions', '/bookmarks', '/accounts', '/apps/ghreview', '/settings']);
	});
});
