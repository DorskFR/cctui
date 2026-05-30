import { browser } from '$app/environment';

/** localStorage-backed draft text (composer per session, spawn form). */
export const drafts = {
	get(key: string): string {
		return browser ? (localStorage.getItem(key) ?? '') : '';
	},
	set(key: string, value: string) {
		if (!browser) return;
		if (value) localStorage.setItem(key, value);
		else localStorage.removeItem(key);
	},
	clear(key: string) {
		if (browser) localStorage.removeItem(key);
	}
};

export const composerKey = (sessionId: string) => `cctui_draft_${sessionId}`;
export const SPAWN_DRAFT = 'cctui_spawn_draft';
export const LAST_MACHINE = 'cctui_last_machine';
export const VIEW_OPTS = 'cctui_view_opts';
export const LIST_DENSITY = 'cctui_list_density';
