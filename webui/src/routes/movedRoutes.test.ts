import { describe, expect, it } from 'vitest';
import { isRedirect } from '@sveltejs/kit';
import { load as loadAccess } from './access/+page';
import { load as loadDispatchers } from './dispatchers/+page';
import { load as loadEvents } from './events/+page';
import { MOVED_ROUTES } from './movedRoutes';
import { isSettingsPage } from '$lib/components/organisms/settings/settings.logic';

function target(load: () => void): { status: number; location: string } {
	try {
		load();
	} catch (e) {
		if (isRedirect(e)) return { status: e.status, location: e.location };
		throw e;
	}
	throw new Error('expected a redirect');
}

describe('the screens that moved under Settings', () => {
	it.each([
		['/access', loadAccess],
		['/dispatchers', loadDispatchers],
		['/events', loadEvents]
	] as const)('forwards %s permanently, on the server', (from, load) => {
		expect(target(load)).toEqual({ status: 308, location: MOVED_ROUTES[from] });
	});

	it('points every old path at a settings page that exists', () => {
		for (const to of Object.values(MOVED_ROUTES)) {
			expect(to.startsWith('/settings/')).toBe(true);
			expect(isSettingsPage(to.slice('/settings/'.length))).toBe(true);
		}
	});
});
