import type { Feature } from '@bindings/Feature';

// Read-only lists shown under Settings › Sessions, the page map, and the
// text-normalizing helper the page filter uses.

// Mirrors the daemon's compiled STALL_PHRASES (crates/cctui-daemon/src/whipstop.rs)
// so users can see what `extend` extends — kept in sync by hand.
export const BUILTIN_STALL_PHRASES = [
	'out of scope',
	'not in scope',
	'beyond the scope',
	'left this for',
	'for a follow-up',
	'next session',
	'future session',
	'can be done later',
	'punting on',
	'pre-existing issue',
	'stopping here',
	'pausing here',
	'good stopping point',
	'natural stopping point',
	'good place to stop',
	'good checkpoint',
	'handing this back',
	'handing it back',
	'over to you',
	'your call',
	'let me know if',
	'let me know how',
	'feel free to',
	'ready for your review',
	'ready for review',
	'for your review',
	'waiting for your',
	'would you like me to',
	'do you want me to',
	'want me to',
	'shall i',
	'should i proceed',
	"if you'd like",
	'happy to continue',
	'happy to keep going'
];

/** Lowercase, strip diacritics: "Réglages" matches "reglages" and vice versa. */
export function normalizeForFilter(s: string): string {
	return s
		.toLowerCase()
		.normalize('NFD')
		.replace(/[̀-ͯ]/g, '');
}

/**
 * Fuzzy row match: every whitespace-separated word of the query must occur in
 * the text, in any order ("passkey enrol" finds "Enrol a passkey"). Rows carry
 * whole paragraphs of help copy, so a looser subsequence match would light up
 * nearly every row for short queries. An empty query matches everything.
 */
export function fuzzyMatches(text: string, query: string): boolean {
	const words = normalizeForFilter(query).split(/\s+/).filter(Boolean);
	if (words.length === 0) return true;
	const hay = normalizeForFilter(text);
	return words.every((w) => hay.includes(w));
}

/**
 * Apply a free-text filter to the rendered settings tree: a row stays when its
 * text matches the query (see `fuzzyMatches`), a group stays when it keeps a row, a section when it
 * keeps a group. Without a query everything shows, row-less empty states included. Returns the number of visible rows so the page can show an
 * empty state. DOM-driven on purpose — the rows already carry their localized
 * copy, so there is no second catalogue to keep in sync. Visibility goes
 * through `style.display` rather than `hidden`: the rows set their own
 * `display`, and Svelte prunes a `[hidden]` selector no template ever sets.
 */
function show(el: HTMLElement, on: boolean) {
	el.style.display = on ? '' : 'none';
}
export function isFiltered(el: HTMLElement): boolean {
	return el.style.display === 'none';
}

export function applySettingsFilter(root: ParentNode, query: string): number {
	const q = query.trim();
	let visible = 0;
	for (const section of root.querySelectorAll<HTMLElement>('[data-setting-section]')) {
		let sectionAny = false;
		for (const group of section.querySelectorAll<HTMLElement>('[data-setting-group]')) {
			let groupAny = false;
			for (const row of group.querySelectorAll<HTMLElement>('[data-setting-row]')) {
				const match = fuzzyMatches(row.textContent ?? '', q);
				show(row, match);
				if (match) {
					groupAny = true;
					visible++;
				}
			}
			show(group, groupAny || !q);
			if (groupAny) sectionAny = true;
		}
		show(section, sectionAny || !q);
	}
	return visible;
}

export const SETTINGS_PAGES = [
	'interface',
	'accounts',
	'sessions',
	'notifications',
	'plugins',
	'instance'
] as const;

export type SettingsPage = (typeof SETTINGS_PAGES)[number];

export const SETTINGS_SECTIONS: Record<SettingsPage, readonly string[]> = {
	interface: ['theme', 'layout', 'conversation-display', 'help'],
	accounts: ['profile', 'security', 'machines', 'provider-accounts', 'pools', 'sharing'],
	sessions: ['launch', 'conversation', 'context', 'privacy', 'automation'],
	notifications: ['notifications', 'voice'],
	plugins: ['plugins'],
	instance: [
		'instance-general',
		'instance-users',
		'instance-machines',
		'instance-limits',
		'instance-gateway',
		'instance-integrations',
		'instance-plugins'
	]
};

export const EVENTS_PAGE = 'events';

const PAGE_FEATURE: Partial<Record<SettingsPage, Feature>> = {
	plugins: 'plugins'
};

/** The feature a page belongs to, `null` for a page that is always there. */
export function settingsPageFeature(page: string | null | undefined): Feature | null {
	return isSettingsPage(page) ? (PAGE_FEATURE[page] ?? null) : null;
}

export function availableSettingsPages(
	has: (feature: Feature) => boolean,
	isAdmin = true
): SettingsPage[] {
	return SETTINGS_PAGES.filter((p) => {
		if (p === 'instance' && !isAdmin) return false;
		const feature = PAGE_FEATURE[p];
		return !feature || has(feature);
	});
}

export const DEFAULT_SETTINGS_PAGE: SettingsPage = 'interface';

export const SETTINGS_SCOPES = ['you', 'instance'] as const;
export type SettingsScope = (typeof SETTINGS_SCOPES)[number];

export function settingsScope(page: SettingsPage): SettingsScope {
	return page === 'instance' ? 'instance' : 'you';
}

export function pagesInScope(scope: SettingsScope): SettingsPage[] {
	return SETTINGS_PAGES.filter((p) => settingsScope(p) === scope);
}

export function isSettingsPage(slug: string | null | undefined): slug is SettingsPage {
	return !!slug && (SETTINGS_PAGES as readonly string[]).includes(slug);
}

export function settingsHref(page: SettingsPage, section?: string): string {
	return section ? `/settings/${page}#${section}` : `/settings/${page}`;
}

export const LEGACY_PAGES: Record<string, string> = {
	appearance: settingsHref('interface', 'theme'),
	monitoring: settingsHref('interface', 'layout'),
	guides: settingsHref('interface', 'help'),
	storage: settingsHref('interface', 'help'),
	security: settingsHref('accounts', 'security'),
	macros: settingsHref('sessions', 'launch'),
	execution: settingsHref('sessions', 'launch'),
	context: settingsHref('sessions', 'context'),
	privacy: settingsHref('sessions', 'privacy'),
	voice: settingsHref('notifications', 'voice'),
	users: settingsHref('instance', 'instance-users'),
	dispatchers: settingsHref('instance', 'instance-users'),
	uploads: settingsHref('instance', 'instance-limits')
};

export function settingsRedirect(slug: string | null | undefined): string | null {
	if (isSettingsPage(slug) || slug === EVENTS_PAGE) return null;
	return (slug && LEGACY_PAGES[slug]) || settingsHref(DEFAULT_SETTINGS_PAGE);
}

export function settingsRouteSlugs(): string[] {
	return [...SETTINGS_PAGES, EVENTS_PAGE, ...Object.keys(LEGACY_PAGES)];
}

const HASH_ALIASES: Record<string, string> = {
	storage: settingsHref('interface', 'help'),
	version: settingsHref('interface', 'help'),
	'net-stats': settingsHref('interface', 'help'),
	onboarding: settingsHref('interface', 'help'),
	tour: settingsHref('interface', 'help'),
	guides: settingsHref('interface', 'help'),
	appearance: settingsHref('interface', 'theme'),
	language: settingsHref('interface', 'theme'),
	spawn: settingsHref('interface', 'layout'),
	resources: settingsHref('interface', 'layout'),
	monitoring: settingsHref('interface', 'layout'),
	passkeys: settingsHref('accounts', 'security'),
	harness: settingsHref('sessions', 'launch'),
	execution: settingsHref('sessions', 'launch'),
	macros: settingsHref('sessions', 'launch'),
	redaction: settingsHref('sessions', 'privacy'),
	secrets: settingsHref('sessions', 'privacy'),
	memory: settingsHref('sessions', 'context'),
	notify: settingsHref('notifications', 'notifications'),
	skills: settingsHref('plugins'),
	'passkey-sign-in': settingsHref('instance', 'instance-general'),
	'self-update': settingsHref('instance', 'instance-general'),
	'upload-caps': settingsHref('instance', 'instance-limits'),
	uploads: settingsHref('instance', 'instance-limits'),
	'spawn-limits': settingsHref('instance', 'instance-limits'),
	'event-retention': settingsHref('instance', 'instance-limits'),
	upstreams: settingsHref('instance', 'instance-gateway'),
	access: settingsHref('instance', 'instance-users'),
	users: settingsHref('instance', 'instance-users'),
	keys: settingsHref('instance', 'instance-users'),
	machines: settingsHref('instance', 'instance-users'),
	tokens: settingsHref('instance', 'instance-users'),
	dispatchers: settingsHref('instance', 'instance-users')
};

export function hrefForHash(hash: string | null | undefined): string {
	const id = (hash ?? '').replace(/^#/, '').trim().toLowerCase();
	if (isSettingsPage(id)) return settingsHref(id);
	if (HASH_ALIASES[id]) return HASH_ALIASES[id];
	for (const page of SETTINGS_PAGES) {
		if (SETTINGS_SECTIONS[page].includes(id)) return settingsHref(page, id);
	}
	return settingsHref(DEFAULT_SETTINGS_PAGE);
}

export const NON_ADMIN_INSTANCE_FALLBACK = settingsHref('accounts', 'profile');

/**
 * First page still holding a visible row once `applySettingsFilter` has run
 * over the whole tree — where the search box jumps to.
 */
export function firstMatchingPage(root: ParentNode): SettingsPage | null {
	for (const el of root.querySelectorAll<HTMLElement>('[data-settings-page]')) {
		const slug = el.dataset.settingsPage;
		if (!isSettingsPage(slug)) continue;
		for (const row of el.querySelectorAll<HTMLElement>('[data-setting-row]')) {
			if (!isFiltered(row)) return slug;
		}
	}
	return null;
}

export function firstMatchingRow(root: ParentNode, page: SettingsPage): HTMLElement | null {
	const el = root.querySelector<HTMLElement>(`[data-settings-page="${page}"]`);
	if (!el) return null;
	for (const row of el.querySelectorAll<HTMLElement>('[data-setting-row]')) {
		if (!isFiltered(row)) return row;
	}
	return null;
}
