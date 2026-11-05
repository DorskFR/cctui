// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest';
import {
	applySettingsFilter,
	DEFAULT_SETTINGS_PAGE,
	firstMatchingPage,
	firstMatchingRow,
	fuzzyMatches,
	isFiltered,
	isSettingsPage,
	LEGACY_PAGES,
	legacySettingsTarget,
	normalizeForFilter,
	pageOfAnchor,
	SETTINGS_ANCHORS,
	SETTINGS_PAGES,
	settingsHref,
	settingsLocationForHash,
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

describe('area map', () => {
	it('shows the five areas plus the admin-only Instance, Interface first', () => {
		expect([...SETTINGS_PAGES]).toEqual([
			'interface',
			'accounts',
			'sessions',
			'notifications',
			'plugins',
			'instance'
		]);
		expect(SETTINGS_PAGES[0]).toBe(DEFAULT_SETTINGS_PAGE);
		expect(SETTINGS_PAGES.filter((p) => settingsScope(p) === 'instance')).toEqual(['instance']);
		expect(isSettingsPage('nope')).toBe(false);
		expect(isSettingsPage(undefined)).toBe(false);
	});

	it('links an area and one of its subsections', () => {
		expect(settingsHref('accounts')).toBe('/settings/accounts');
		expect(settingsHref('interface', 'theme')).toBe('/settings/interface#theme');
	});

	it('keeps every anchor unique across areas, since all areas stay mounted', () => {
		const all = Object.values(SETTINGS_ANCHORS).flat();
		expect(new Set(all).size).toBe(all.length);
		for (const page of SETTINGS_PAGES) {
			for (const anchor of SETTINGS_ANCHORS[page]) expect(pageOfAnchor(anchor)).toBe(page);
		}
	});

	it('resolves the anchors other screens link to', () => {
		for (const href of [
			'/settings/interface#theme',
			'/settings/sessions#profiles',
			'/settings/accounts#providers',
			'/settings/instance#users',
			'/settings/instance#dispatchers'
		]) {
			const [path, anchor] = href.split('#');
			expect(pageOfAnchor(anchor)).toBe(path.slice('/settings/'.length));
		}
	});
});

describe('old settings URLs', () => {
	const OLD_PAGES = [
		'appearance',
		'sessions',
		'macros',
		'context',
		'plugins',
		'execution',
		'privacy',
		'notifications',
		'monitoring',
		'events',
		'security',
		'guides',
		'users',
		'dispatchers',
		'instance',
		'uploads'
	];

	function resolves(href: string): boolean {
		if (href === '/events') return true;
		const [path, anchor] = href.split('#');
		const page = path.slice('/settings/'.length);
		if (!isSettingsPage(page)) return false;
		return anchor === undefined || pageOfAnchor(anchor) === page;
	}

	it('sends every one of the 16 old pages to a location that exists', () => {
		expect(OLD_PAGES).toHaveLength(16);
		for (const slug of OLD_PAGES) {
			const to = legacySettingsTarget(slug);
			if (isSettingsPage(slug)) expect(to).toBeNull();
			else expect(to && resolves(to), `${slug} → ${to}`).toBe(true);
		}
	});

	it('maps the pages that moved into an area', () => {
		expect(legacySettingsTarget('appearance')).toBe('/settings/interface#theme');
		expect(legacySettingsTarget('security')).toBe('/settings/accounts#security');
		expect(legacySettingsTarget('users')).toBe('/settings/instance#users');
		expect(legacySettingsTarget('dispatchers')).toBe('/settings/instance#dispatchers');
		expect(legacySettingsTarget('uploads')).toBe('/settings/instance#uploads');
		expect(legacySettingsTarget('events')).toBe('/events');
		expect(legacySettingsTarget('guides')).toBe('/settings/interface#guides');
	});

	it('sends an unknown slug to the first area', () => {
		expect(legacySettingsTarget('nope')).toBe('/settings/interface');
		expect(legacySettingsTarget(undefined)).toBe('/settings/interface');
	});

	it('points every legacy entry at a location that exists', () => {
		for (const [slug, to] of Object.entries(LEGACY_PAGES)) {
			expect(resolves(to), `${slug} → ${to}`).toBe(true);
		}
	});

	it('keeps old /settings#anchor deep links working', () => {
		expect(settingsLocationForHash('#security')).toBe('/settings/accounts#security');
		expect(settingsLocationForHash('passkeys')).toBe('/settings/accounts#security');
		expect(settingsLocationForHash('#Storage')).toBe('/settings/interface#storage');
		expect(settingsLocationForHash('#passkey-sign-in')).toBe('/settings/instance#general');
		expect(settingsLocationForHash('#self-update')).toBe('/settings/instance#self-update');
		expect(settingsLocationForHash('#upstreams')).toBe('/settings/instance#upstreams');
		expect(settingsLocationForHash('#spawn-limits')).toBe('/settings/instance#spawn-limits');
		expect(settingsLocationForHash('#redaction')).toBe('/settings/sessions#privacy');
		expect(settingsLocationForHash('#memory')).toBe('/settings/sessions#context');
		expect(settingsLocationForHash('#skills')).toBe('/settings/plugins');
		expect(settingsLocationForHash('#access')).toBe('/settings/instance#users');
		expect(settingsLocationForHash('#tokens')).toBe('/settings/instance#users');
		expect(settingsLocationForHash('#appearance')).toBe('/settings/interface#theme');
		expect(settingsLocationForHash('#sessions')).toBe('/settings/sessions');
		expect(settingsLocationForHash('')).toBe('/settings/interface');
		expect(settingsLocationForHash(null)).toBe('/settings/interface');
		expect(settingsLocationForHash('#whatever')).toBe('/settings/interface');
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
