import { describe, expect, it } from 'vitest';
import { pageOfAnchor } from './settings.logic';
import { searchSections, SETTINGS_SECTIONS } from './settingsIndex';

describe('settings section index', () => {
	it('finds the passkeys under Accounts, with the area as a breadcrumb', () => {
		const hits = searchSections('passkey', false);
		expect(hits).toContainEqual(
			expect.objectContaining({ page: 'accounts', anchor: 'security', area: 'Accounts' })
		);
	});

	it('finds sections that hold no plain setting rows', () => {
		expect(searchSections('provider', false).map((h) => h.anchor)).toContain('providers');
		expect(searchSections('users', true).map((h) => h.anchor)).toContain('users');
	});

	it('keeps Instance sections out of a non-admin search', () => {
		expect(searchSections('users', false).some((h) => h.page === 'instance')).toBe(false);
	});

	it('returns nothing for a blank query', () => {
		expect(searchSections('  ', true)).toEqual([]);
	});

	it('points every entry at an anchor of its own area', () => {
		for (const s of SETTINGS_SECTIONS) expect(pageOfAnchor(s.anchor)).toBe(s.page);
	});
});
