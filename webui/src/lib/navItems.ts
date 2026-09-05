import { ghreviewUrl } from '$lib/config';
import { m } from '$lib/paraglide/messages';

export interface NavItemSpec {
	href: string;
	label: string;
	icon: string;
}

export function navItems(): NavItemSpec[] {
	return [
		{ href: '/', label: m.nav_overview(), icon: '◧' },
		{ href: '/sessions', label: m.nav_sessions(), icon: '◰' },
		{ href: '/users', label: m.nav_users(), icon: '◍' },
		{ href: '/accounts', label: m.nav_accounts(), icon: '◉' },
		...(ghreviewUrl() !== null ? [{ href: '/github', label: m.nav_github(), icon: '◐' }] : []),
		{ href: '/settings', label: m.nav_settings(), icon: '⚙' }
	];
}

export function isNavActive(href: string, pathname: string): boolean {
	return href === '/' ? pathname === '/' : pathname.startsWith(href);
}
