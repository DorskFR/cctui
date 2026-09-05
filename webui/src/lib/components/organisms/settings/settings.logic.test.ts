import { afterEach, describe, expect, it } from 'vitest';
import {
	applySettingsFilter,
	DEFAULT_SETTINGS_PAGE,
	firstMatchingPage,
	firstMatchingRow,
	isSettingsPage,
	normalizeForFilter,
	pageForHash,
	pagerNeighbours,
	SETTINGS_PAGES,
	settingsHref
} from './settings.logic';

function tree(): HTMLElement {
	const root = document.createElement('div');
	root.innerHTML = `
		<div data-settings-page="appearance">
			<section data-setting-section>
				<div data-setting-group>
					<div data-setting-row>Theme</div>
					<div data-setting-row>Interface language</div>
				</div>
			</section>
		</div>
		<div data-settings-page="security">
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

describe('page map', () => {
	it('starts on the default page and knows every slug', () => {
		expect(SETTINGS_PAGES[0]).toBe(DEFAULT_SETTINGS_PAGE);
		expect(SETTINGS_PAGES.every(isSettingsPage)).toBe(true);
		expect(isSettingsPage('nope')).toBe(false);
		expect(isSettingsPage(undefined)).toBe(false);
	});

	it('maps a page slug to its route', () => {
		expect(settingsHref('security')).toBe('/settings/security');
	});

	it('keeps old /settings#anchor deep links working', () => {
		expect(pageForHash('#security')).toBe('security');
		expect(pageForHash('passkeys')).toBe('security');
		expect(pageForHash('#Storage')).toBe('instance');
		expect(pageForHash('#self-update')).toBe('instance');
		expect(pageForHash('#redaction')).toBe('privacy');
		expect(pageForHash('')).toBe(DEFAULT_SETTINGS_PAGE);
		expect(pageForHash(null)).toBe(DEFAULT_SETTINGS_PAGE);
		expect(pageForHash('#whatever')).toBe(DEFAULT_SETTINGS_PAGE);
	});

	it('pages in order, with no neighbour past either end', () => {
		const first = SETTINGS_PAGES[0];
		const last = SETTINGS_PAGES[SETTINGS_PAGES.length - 1];
		expect(pagerNeighbours(first).prev).toBeNull();
		expect(pagerNeighbours(first).next).toBe(SETTINGS_PAGES[1]);
		expect(pagerNeighbours(last).next).toBeNull();
		expect(pagerNeighbours('privacy')).toEqual({ prev: 'execution', next: 'notifications' });
	});
});

describe('cross-page search', () => {
	it('filters rows on every page at once', () => {
		const root = tree();
		expect(applySettingsFilter(root, 'passkeys')).toBe(1);
		expect(firstMatchingPage(root)).toBe('security');
		expect(firstMatchingRow(root, 'security')?.textContent).toBe('Passkeys');
		expect(firstMatchingRow(root, 'appearance')).toBeNull();
	});

	it('jumps to the first page in navigation order that still matches', () => {
		const root = tree();
		applySettingsFilter(root, 'e');
		expect(firstMatchingPage(root)).toBe('appearance');
		expect(firstMatchingRow(root, 'appearance')?.textContent).toBe('Theme');
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
		expect(firstMatchingPage(root)).toBe('appearance');
	});
});
