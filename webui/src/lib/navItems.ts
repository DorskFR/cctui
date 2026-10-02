import type { IconName } from '@dorsk/tsumikit';
import { m } from '$lib/paraglide/messages';

export interface NavItemSpec {
	href: string;
	label: string;
	iconName: IconName;
}

export interface NavGates {
	/** Enabled page plugins, appended before Settings. */
	pages?: { href: string; label: string; iconName: IconName }[];
}

export function navItems(gates: NavGates = {}): NavItemSpec[] {
	return [
		{ href: '/', label: m.nav_overview(), iconName: 'layout-grid' },
		{ href: '/sessions', label: m.nav_sessions(), iconName: 'list' },
		{ href: '/bookmarks', label: m.nav_bookmarks(), iconName: 'bookmark' },
		...(gates.pages ?? []),
		{ href: '/settings', label: m.nav_settings(), iconName: 'settings' }
	];
}

/** Guide anchor key for a nav item. A target path splits on `/`, so the href
 *  itself cannot be the key. */
export function navKey(href: string): string {
	return href === '/' ? 'overview' : href.replace(/^\//, '').replace(/\//g, '-');
}

export function isNavActive(href: string, pathname: string): boolean {
	return href === '/' ? pathname === '/' : pathname.startsWith(href);
}
