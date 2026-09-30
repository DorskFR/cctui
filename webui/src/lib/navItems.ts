import type { IconName } from '@dorsk/tsumikit';
import { m } from '$lib/paraglide/messages';

export interface NavItemSpec {
	href: string;
	label: string;
	icon: string;
	/** Set instead of `icon` by plugin entries, whose manifest names a kit icon. */
	iconName?: IconName;
}

export interface NavGates {
	/** Enabled page plugins, appended before Settings. */
	pages?: { href: string; label: string; iconName: IconName }[];
}

export function navItems(gates: NavGates = {}): NavItemSpec[] {
	return [
		{ href: '/', label: m.nav_overview(), icon: '◧' },
		{ href: '/sessions', label: m.nav_sessions(), icon: '◰' },
		{ href: '/tiles', label: m.nav_tiles(), icon: '◫' },
		{ href: '/bookmarks', label: m.nav_bookmarks(), icon: '◈' },
		{ href: '/access', label: m.nav_access(), icon: '◍' },
		{ href: '/accounts', label: m.nav_accounts(), icon: '◉' },
		...(gates.pages ?? []).map((p) => ({ href: p.href, label: p.label, icon: '', iconName: p.iconName })),
		{ href: '/settings', label: m.nav_settings(), icon: '⚙' }
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
