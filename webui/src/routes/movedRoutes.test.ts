import { describe, expect, it } from 'vitest';
import { isRedirect } from '@sveltejs/kit';
import { load as loadAccess } from './access/+page';
import { load as loadDispatchers } from './dispatchers/+page';
import { load as loadSettingsPage } from './settings/[page]/+page';
import { MOVED_ROUTES } from './movedRoutes';
import {
	isSettingsPage,
	pageOfAnchor
} from '$lib/components/organisms/settings/settings.logic';

function target(load: () => void): { status: number; location: string } | null {
	try {
		load();
	} catch (e) {
		if (isRedirect(e)) return { status: e.status, location: e.location };
		throw e;
	}
	return null;
}

describe('the screens that moved under Settings', () => {
	it.each([
		['/access', loadAccess],
		['/dispatchers', loadDispatchers]
	] as const)('forwards %s permanently, on the server', (from, load) => {
		expect(target(load)).toEqual({ status: 308, location: MOVED_ROUTES[from] });
	});

	it('points every old path at a settings subsection that exists', () => {
		for (const to of Object.values(MOVED_ROUTES)) {
			const [path, anchor] = to.split('#');
			const page = path.slice('/settings/'.length);
			expect(isSettingsPage(page)).toBe(true);
			expect(pageOfAnchor(anchor)).toBe(page);
		}
	});

	it('no longer forwards /events: it is a page of its own', () => {
		expect(Object.keys(MOVED_ROUTES)).not.toContain('/events');
	});
});

describe('/settings/[page]', () => {
	const visit = (page: string) => target(() => loadSettingsPage({ params: { page } }));

	it('renders the current areas', () => {
		for (const page of ['interface', 'accounts', 'sessions', 'notifications', 'plugins', 'instance']) {
			expect(visit(page)).toBeNull();
		}
	});

	it('forwards an old page permanently to its new location', () => {
		expect(visit('appearance')).toEqual({ status: 308, location: '/settings/interface#theme' });
		expect(visit('security')).toEqual({ status: 308, location: '/settings/accounts#security' });
		expect(visit('users')).toEqual({ status: 308, location: '/settings/instance#users' });
		expect(visit('events')).toEqual({ status: 308, location: '/events' });
	});

	it('forwards an unknown page to the first area', () => {
		expect(visit('nope')).toEqual({ status: 308, location: '/settings/interface' });
	});
});
