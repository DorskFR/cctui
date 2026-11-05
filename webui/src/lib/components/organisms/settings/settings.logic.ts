// The Settings area map and its redirects, the cross-area filter, and the
// read-only stall-phrase list shown under Sessions › Automation.

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
 * keeps a group. A section in `keepSections` (matched by name in the section index) stays whole. Without a query everything shows, row-less empty states included. Returns the number of visible rows so the page can show an
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

export function applySettingsFilter(
	root: ParentNode,
	query: string,
	keepSections: ReadonlySet<string> = new Set()
): number {
	const q = query.trim();
	let visible = 0;
	for (const section of root.querySelectorAll<HTMLElement>('[data-setting-section]')) {
		if (q && keepSections.has(section.id)) {
			show(section, true);
			for (const el of section.querySelectorAll<HTMLElement>('[data-setting-group], [data-setting-row]'))
				show(el, true);
			visible += Math.max(1, section.querySelectorAll('[data-setting-row]').length);
			continue;
		}
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

/**
 * The routed Settings areas, in navigation order. Every "you" area comes
 * before the admin-only Instance, so the nav renders one heading per scope
 * without reordering.
 */
export const SETTINGS_PAGES = [
	'interface',
	'accounts',
	'sessions',
	'notifications',
	'plugins',
	'instance'
] as const;

export type SettingsPage = (typeof SETTINGS_PAGES)[number];

export const DEFAULT_SETTINGS_PAGE: SettingsPage = 'interface';

export const SETTINGS_SCOPES = ['you', 'instance'] as const;
export type SettingsScope = (typeof SETTINGS_SCOPES)[number];

export function settingsScope(page: SettingsPage): SettingsScope {
	return page === 'instance' ? 'instance' : 'you';
}

/**
 * The subsection anchors of each area. Every area stays mounted, so an anchor
 * is unique across all of them, not just within its own page.
 */
export const SETTINGS_ANCHORS = {
	interface: ['theme', 'layout', 'monitoring', 'conversation-display', 'guides', 'storage'],
	accounts: ['profile', 'security', 'machines', 'providers'],
	sessions: ['profiles', 'macros', 'conversation', 'context', 'privacy', 'automation'],
	notifications: ['notifications', 'voice'],
	plugins: ['plugins'],
	instance: [
		'general',
		'self-update',
		'users',
		'dispatchers',
		'harnesses',
		'limits',
		'spawn-limits',
		'uploads',
		'event-retention',
		'network',
		'upstreams',
		'integrations',
		'speech',
		'instance-plugins'
	]
} as const satisfies Record<SettingsPage, readonly string[]>;

export function settingsHref(page: SettingsPage, anchor?: string): string {
	return anchor ? `/settings/${page}#${anchor}` : `/settings/${page}`;
}

/** Former `/settings/<slug>` pages and where their content lives now. */
export const LEGACY_PAGES: Readonly<Record<string, string>> = {
	appearance: settingsHref('interface', 'theme'),
	macros: settingsHref('sessions', 'macros'),
	context: settingsHref('sessions', 'context'),
	execution: settingsHref('sessions', 'profiles'),
	privacy: settingsHref('sessions', 'privacy'),
	monitoring: settingsHref('interface', 'monitoring'),
	events: '/events',
	security: settingsHref('accounts', 'security'),
	guides: settingsHref('interface', 'guides'),
	users: settingsHref('instance', 'users'),
	dispatchers: settingsHref('instance', 'dispatchers'),
	uploads: settingsHref('instance', 'uploads')
};

/** Anchors of the old single-page Settings that are not anchors any more. */
const HASH_ALIASES: Readonly<Record<string, string>> = {
	version: settingsHref('instance', 'general'),
	'passkey-sign-in': settingsHref('instance', 'general'),
	'upload-caps': settingsHref('instance', 'uploads'),
	'net-stats': settingsHref('instance', 'general'),
	language: settingsHref('interface', 'theme'),
	spawn: settingsHref('interface', 'layout'),
	harness: settingsHref('sessions', 'profiles'),
	redaction: settingsHref('sessions', 'privacy'),
	secrets: settingsHref('sessions', 'privacy'),
	notify: settingsHref('notifications', 'notifications'),
	resources: settingsHref('interface', 'monitoring'),
	passkeys: settingsHref('accounts', 'security'),
	memory: settingsHref('sessions', 'context'),
	skills: settingsHref('plugins'),
	onboarding: settingsHref('interface', 'guides'),
	tour: settingsHref('interface', 'guides'),
	access: settingsHref('instance', 'users'),
	keys: settingsHref('instance', 'users'),
	tokens: settingsHref('instance', 'users')
};

export function isSettingsPage(slug: string | null | undefined): slug is SettingsPage {
	return !!slug && (SETTINGS_PAGES as readonly string[]).includes(slug);
}

/** The area owning a subsection anchor, if any. */
export function pageOfAnchor(anchor: string): SettingsPage | null {
	for (const page of SETTINGS_PAGES) {
		if ((SETTINGS_ANCHORS[page] as readonly string[]).includes(anchor)) return page;
	}
	return null;
}

/** Where an old `/settings/<slug>` now lives; `null` when the slug is current. */
export function legacySettingsTarget(slug: string | null | undefined): string | null {
	if (isSettingsPage(slug)) return null;
	return (slug && LEGACY_PAGES[slug]) || settingsHref(DEFAULT_SETTINGS_PAGE);
}

/** `/settings#passkeys` and friends keep working: hash → its current location. */
export function settingsLocationForHash(hash: string | null | undefined): string {
	const id = (hash ?? '').replace(/^#/, '').trim().toLowerCase();
	if (!id) return settingsHref(DEFAULT_SETTINGS_PAGE);
	if (isSettingsPage(id)) return settingsHref(id);
	const page = pageOfAnchor(id);
	if (page) return settingsHref(page, id);
	return LEGACY_PAGES[id] ?? HASH_ALIASES[id] ?? settingsHref(DEFAULT_SETTINGS_PAGE);
}

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
