// Read-only lists shown under Settings › Execution / Privacy, plus the
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

// Mirrors the daemon's built-in secret detectors (category names as they
// appear in `[REDACTED:<category>]` markers).
export const BUILTIN_SCRUB_CATEGORIES = [
	'github_token',
	'github_pat',
	'npm_token',
	'anthropic_key',
	'aws_access_key',
	'vault_token',
	'gitlab_token',
	'slack_token',
	'youtrack_token',
	'bitwarden_token',
	'cctui_token',
	'ccipat',
	'private_key',
	'jwt',
	'db_url_password'
];

/** Lowercase, strip diacritics: "Réglages" matches "reglages" and vice versa. */
export function normalizeForFilter(s: string): string {
	return s
		.toLowerCase()
		.normalize('NFD')
		.replace(/[̀-ͯ]/g, '');
}

/**
 * Apply a free-text filter to the rendered settings tree: a row stays when its
 * text contains the query, a group stays when it keeps a row, a section when it
 * keeps a group. Returns the number of visible rows so the page can show an
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
	const q = normalizeForFilter(query.trim());
	let visible = 0;
	for (const section of root.querySelectorAll<HTMLElement>('[data-setting-section]')) {
		let sectionAny = false;
		for (const group of section.querySelectorAll<HTMLElement>('[data-setting-group]')) {
			let groupAny = false;
			for (const row of group.querySelectorAll<HTMLElement>('[data-setting-row]')) {
				const match = !q || normalizeForFilter(row.textContent ?? '').includes(q);
				show(row, match);
				if (match) {
					groupAny = true;
					visible++;
				}
			}
			show(group, groupAny);
			if (groupAny) sectionAny = true;
		}
		show(section, sectionAny);
	}
	return visible;
}

/** The routed Settings pages, in navigation order. */
export const SETTINGS_PAGES = [
	'appearance',
	'sessions',
	'execution',
	'privacy',
	'notifications',
	'monitoring',
	'security',
	'instance'
] as const;

export type SettingsPage = (typeof SETTINGS_PAGES)[number];

export const DEFAULT_SETTINGS_PAGE: SettingsPage = 'appearance';

/** Anchors older deep links point at, and the page that now owns them. */
const HASH_ALIASES: Record<string, SettingsPage> = {
	storage: 'instance',
	version: 'instance',
	'self-update': 'instance',
	'net-stats': 'instance',
	theme: 'appearance',
	language: 'appearance',
	spawn: 'sessions',
	harness: 'execution',
	redaction: 'privacy',
	secrets: 'privacy',
	notify: 'notifications',
	resources: 'monitoring',
	passkeys: 'security'
};

export function isSettingsPage(slug: string | null | undefined): slug is SettingsPage {
	return !!slug && (SETTINGS_PAGES as readonly string[]).includes(slug);
}

/** `/settings#passkeys` and friends keep working: hash → owning page. */
export function pageForHash(hash: string | null | undefined): SettingsPage {
	const id = (hash ?? '').replace(/^#/, '').trim().toLowerCase();
	if (isSettingsPage(id)) return id;
	return HASH_ALIASES[id] ?? DEFAULT_SETTINGS_PAGE;
}

export function settingsHref(page: SettingsPage): string {
	return `/settings/${page}`;
}

export function pagerNeighbours(page: SettingsPage): {
	prev: SettingsPage | null;
	next: SettingsPage | null;
} {
	const i = SETTINGS_PAGES.indexOf(page);
	return {
		prev: i > 0 ? SETTINGS_PAGES[i - 1] : null,
		next: i >= 0 && i < SETTINGS_PAGES.length - 1 ? SETTINGS_PAGES[i + 1] : null
	};
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
