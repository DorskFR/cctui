// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest';
import {
	applySettingsFilter,
	availableSettingsPages,
	DEFAULT_SETTINGS_PAGE,
	EVENTS_PAGE,
	firstMatchingPage,
	firstMatchingRow,
	fuzzyMatches,
	hrefForHash,
	isFiltered,
	isSettingsPage,
	LEGACY_PAGES,
	NON_ADMIN_INSTANCE_FALLBACK,
	normalizeForFilter,
	pagesInScope,
	SETTINGS_PAGES,
	SETTINGS_SCOPES,
	SETTINGS_SECTIONS,
	settingsHref,
	settingsPageFeature,
	settingsRedirect,
	settingsRouteSlugs,
	settingsScope
} from './settings.logic';

function tree(): HTMLElement {
	const root = document.createElement('div');
	root.innerHTML = `
		<div data-settings-page="interface">
			<section data-setting-section>
				<div data-setting-group>
					<div data-setting-row>Theme</div>
					<div data-setting-row>Interface language</div>
				</div>
			</section>
		</div>
		<div data-settings-page="accounts">
			<section data-setting-section>
				<div data-setting-group>
					<div data-setting-row>Passkeys</div>
				</div>
			</section>
		</div>
	`;
	document.body.append(root);
	return root;
}

afterEach(() => document.body.replaceChildren());

describe('applySettingsFilter', () => {
	it('keeps row-less sections and groups visible until a query filters them', () => {
		const root = document.createElement('div');
		root.innerHTML = `
			<section data-setting-section id="empty-section"><p>No plugins installed</p></section>
			<section data-setting-section id="empty-group"><div data-setting-group><p>Install from URL</p></div></section>
		`;
		document.body.append(root);
		const section = root.querySelector<HTMLElement>('#empty-section')!;
		const group = root.querySelector<HTMLElement>('#empty-group [data-setting-group]')!;

		applySettingsFilter(root, '');
		expect(isFiltered(section)).toBe(false);
		expect(isFiltered(group)).toBe(false);

		applySettingsFilter(root, 'theme');
		expect(isFiltered(section)).toBe(true);
		expect(isFiltered(group)).toBe(true);
	});
});

describe('page map', () => {
	it('navigates exactly the five areas, then the admin-only Instance', () => {
		expect([...SETTINGS_PAGES]).toEqual([
			'interface',
			'accounts',
			'sessions',
			'notifications',
			'plugins',
			'instance'
		]);
		expect(SETTINGS_PAGES[0]).toBe(DEFAULT_SETTINGS_PAGE);
		expect(SETTINGS_PAGES.every(isSettingsPage)).toBe(true);
		expect(isSettingsPage('nope')).toBe(false);
		expect(isSettingsPage(undefined)).toBe(false);
	});

	it('maps a page and an optional sub-area to its route', () => {
		expect(settingsHref('accounts')).toBe('/settings/accounts');
		expect(settingsHref('accounts', 'security')).toBe('/settings/accounts#security');
	});

	it('gives every sub-area a unique anchor across pages', () => {
		const ids = SETTINGS_PAGES.flatMap((p) => [...SETTINGS_SECTIONS[p]]);
		expect(ids).toEqual([...new Set(ids)]);
	});

	it('groups every page under exactly one scope, Instance last', () => {
		expect(SETTINGS_SCOPES.flatMap(pagesInScope)).toEqual([...SETTINGS_PAGES]);
		expect(pagesInScope('instance')).toEqual(['instance']);
		expect(settingsScope('interface')).toBe('you');
		expect(settingsScope('instance')).toBe('instance');
	});
});

describe('old settings URLs', () => {
	const OLD_PAGES: Record<string, string> = {
		appearance: '/settings/interface#theme',
		monitoring: '/settings/interface#layout',
		guides: '/settings/interface#help',
		storage: '/settings/interface#help',
		security: '/settings/accounts#security',
		macros: '/settings/sessions#launch',
		execution: '/settings/sessions#launch',
		context: '/settings/sessions#context',
		privacy: '/settings/sessions#privacy',
		voice: '/settings/notifications#voice',
		users: '/settings/instance#instance-users',
		dispatchers: '/settings/instance#instance-users',
		uploads: '/settings/instance#instance-limits'
	};

	it.each(Object.entries(OLD_PAGES))('redirects /settings/%s to %s', (slug, target) => {
		expect(settingsRedirect(slug)).toBe(target);
	});

	it('covers every former page that is not a page any more', () => {
		expect(Object.keys(LEGACY_PAGES).sort()).toEqual(Object.keys(OLD_PAGES).sort());
		for (const target of Object.values(LEGACY_PAGES)) {
			const [path, hash] = target.replace('/settings/', '').split('#');
			expect(isSettingsPage(path)).toBe(true);
			expect(SETTINGS_SECTIONS[path as (typeof SETTINGS_PAGES)[number]]).toContain(hash);
		}
	});

	it('serves the current pages and the event log as they are', () => {
		for (const page of SETTINGS_PAGES) expect(settingsRedirect(page)).toBeNull();
		expect(settingsRedirect(EVENTS_PAGE)).toBeNull();
	});

	it('sends an unknown slug to the first page', () => {
		expect(settingsRedirect('nope')).toBe('/settings/interface');
		expect(settingsRedirect('')).toBe('/settings/interface');
		expect(settingsRedirect(undefined)).toBe('/settings/interface');
	});

	it('prerenders every slug the route answers, redirects included', () => {
		const slugs = settingsRouteSlugs();
		for (const slug of [...SETTINGS_PAGES, EVENTS_PAGE, ...Object.keys(OLD_PAGES)])
			expect(slugs).toContain(slug);
	});

	it('sends a non-admin away from Instance to their own identity', () => {
		expect(NON_ADMIN_INSTANCE_FALLBACK).toBe('/settings/accounts#profile');
	});
});

describe('old /settings#anchor links', () => {
	it.each([
		['#passkeys', '/settings/accounts#security'],
		['#Storage', '/settings/interface#help'],
		['#version', '/settings/interface#help'],
		['#passkey-sign-in', '/settings/instance#instance-general'],
		['#self-update', '/settings/instance#instance-general'],
		['#upstreams', '/settings/instance#instance-gateway'],
		['#spawn-limits', '/settings/instance#instance-limits'],
		['#redaction', '/settings/sessions#privacy'],
		['#memory', '/settings/sessions#context'],
		['#skills', '/settings/plugins'],
		['#resources', '/settings/interface#layout'],
		['#access', '/settings/instance#instance-users'],
		['#keys', '/settings/instance#instance-users'],
		['#machines', '/settings/instance#instance-users'],
		['#tokens', '/settings/instance#instance-users']
	])('forwards %s to %s', (hash, target) => {
		expect(hrefForHash(hash)).toBe(target);
	});

	it('takes a page slug or a current sub-area as is', () => {
		expect(hrefForHash('#accounts')).toBe('/settings/accounts');
		expect(hrefForHash('#automation')).toBe('/settings/sessions#automation');
		expect(hrefForHash('instance-limits')).toBe('/settings/instance#instance-limits');
	});

	it('falls back to the first page', () => {
		expect(hrefForHash('')).toBe('/settings/interface');
		expect(hrefForHash(null)).toBe('/settings/interface');
		expect(hrefForHash('#whatever')).toBe('/settings/interface');
	});
});

describe('fuzzyMatches', () => {
	it('matches every query word in any order, ignoring case and diacritics', () => {
		expect(fuzzyMatches('Enrol a passkey', 'passkey enrol')).toBe(true);
		expect(fuzzyMatches('Notify on input needed', 'notify inp')).toBe(true);
		expect(fuzzyMatches('Réglages', 'REGL')).toBe(true);
	});

	it('rejects a query with a word the text lacks', () => {
		expect(fuzzyMatches('Enrol a passkey', 'passkey remove')).toBe(false);
		expect(fuzzyMatches('Theme', 'thme')).toBe(false);
	});

	it('matches everything on an empty or blank query', () => {
		expect(fuzzyMatches('Theme', '')).toBe(true);
		expect(fuzzyMatches('Theme', '   ')).toBe(true);
	});

	it('drives the tree filter word by word', () => {
		const root = tree();
		expect(applySettingsFilter(root, 'language interface')).toBe(1);
		expect(firstMatchingRow(root, 'interface')?.textContent).toBe('Interface language');
	});
});

describe('cross-page search', () => {
	it('filters rows on every page at once', () => {
		const root = tree();
		expect(applySettingsFilter(root, 'passkeys')).toBe(1);
		expect(firstMatchingPage(root)).toBe('accounts');
		expect(firstMatchingRow(root, 'accounts')?.textContent).toBe('Passkeys');
		expect(firstMatchingRow(root, 'interface')).toBeNull();
	});

	it('jumps to the first page in navigation order that still matches', () => {
		const root = tree();
		applySettingsFilter(root, 'e');
		expect(firstMatchingPage(root)).toBe('interface');
		expect(firstMatchingRow(root, 'interface')?.textContent).toBe('Theme');
	});

	it('reports no page when nothing matches', () => {
		const root = tree();
		expect(applySettingsFilter(root, 'zzz')).toBe(0);
		expect(firstMatchingPage(root)).toBeNull();
	});

	it('ignores case and diacritics', () => {
		expect(normalizeForFilter('Réglages')).toBe('reglages');
		const root = tree();
		expect(applySettingsFilter(root, 'THEME')).toBe(1);
		expect(firstMatchingPage(root)).toBe('interface');
	});
});

describe('settings pages by announced features and role', () => {
	it('lists every page with every feature on, for an admin', () => {
		expect(availableSettingsPages(() => true, true)).toEqual([...SETTINGS_PAGES]);
	});

	it('hides Instance from a non-admin', () => {
		expect(availableSettingsPages(() => true, false)).toEqual([
			'interface',
			'accounts',
			'sessions',
			'notifications',
			'plugins'
		]);
	});

	it('drops only the plugins page on a core server', () => {
		expect(availableSettingsPages(() => false)).toEqual([
			'interface',
			'accounts',
			'sessions',
			'notifications',
			'instance'
		]);
	});

	it('names the feature behind each gated page', () => {
		expect(settingsPageFeature('plugins')).toBe('plugins');
		expect(settingsPageFeature('accounts')).toBeNull();
		expect(settingsPageFeature('instance')).toBeNull();
		expect(settingsPageFeature('nope')).toBeNull();
	});
});
