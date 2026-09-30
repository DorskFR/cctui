import { ghreviewUrl } from '$lib/config';
import { m } from '$lib/paraglide/messages';

export interface NavItemSpec {
	href: string;
	label: string;
	icon: string;
}

export interface NavGates {
	hasGithubConnector?: boolean;
}

export function navItems(gates: NavGates = {}): NavItemSpec[] {
	return [
		{ href: '/', label: m.nav_overview(), icon: '◧' },
		{ href: '/sessions', label: m.nav_sessions(), icon: '◰' },
		{ href: '/tiles', label: m.nav_tiles(), icon: '◫' },
		{ href: '/bookmarks', label: m.nav_bookmarks(), icon: '◈' },
		{ href: '/access', label: m.nav_access(), icon: '◍' },
		{ href: '/accounts', label: m.nav_accounts(), icon: '◉' },
		...(ghreviewUrl() !== null && gates.hasGithubConnector === true
			? [{ href: '/github', label: m.nav_github(), icon: '◐' }]
			: []),
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
