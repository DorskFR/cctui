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
export const historyKey = (sessionId: string) => `cctui_history_${sessionId}`;

const HISTORY_MAX = 5;

/** localStorage-backed per-session sent-message history (most-recent-last,
 * capped at HISTORY_MAX). Used for ArrowUp/ArrowDown recall in the composer. */
export const history = {
	get(sessionId: string): string[] {
		if (!browser) return [];
		try {
			const raw = localStorage.getItem(historyKey(sessionId));
			const arr = raw ? JSON.parse(raw) : [];
			return Array.isArray(arr) ? arr.filter((x): x is string => typeof x === 'string') : [];
		} catch {
			return [];
		}
	},
	push(sessionId: string, value: string) {
		if (!browser) return;
		const v = value.trim();
		if (!v) return;
		// De-dupe a repeat of the most recent entry, append, cap to last N.
		const list = this.get(sessionId).filter((x) => x !== v);
		list.push(v);
		const capped = list.slice(-HISTORY_MAX);
		localStorage.setItem(historyKey(sessionId), JSON.stringify(capped));
	},
	clear(sessionId: string) {
		if (browser) localStorage.removeItem(historyKey(sessionId));
	}
};

/** Remove all localStorage tied to a session (draft + sent-message history).
 * Called when a conversation is archived. */
export function clearSessionStorage(sessionId: string) {
	if (!browser) return;
	localStorage.removeItem(composerKey(sessionId));
	localStorage.removeItem(historyKey(sessionId));
}

export const SPAWN_DRAFT = 'cctui_spawn_draft';
export const LAST_MACHINE = 'cctui_last_machine';

/** Spawn settings remembered PER MACHINE (CCT-274): the adapter, per-adapter
 * model + effort, and account last used on a given machine, so the next spawn
 * on e.g. dev1 re-selects what you usually run there. Keyed by machine id. */
export interface MachineSpawnPrefs {
	adapter_id: string;
	model_claude: string;
	model_codex: string;
	effort_claude: string;
	effort_codex: string;
	account: string;
}
const machinePrefsKey = (machineId: string) => `cctui_spawn_prefs_${machineId}`;

export function loadMachinePrefs(machineId: string): Partial<MachineSpawnPrefs> | null {
	if (!browser || !machineId) return null;
	try {
		const raw = localStorage.getItem(machinePrefsKey(machineId));
		return raw ? (JSON.parse(raw) as Partial<MachineSpawnPrefs>) : null;
	} catch {
		return null;
	}
}

export function saveMachinePrefs(machineId: string, prefs: MachineSpawnPrefs) {
	if (!browser || !machineId) return;
	localStorage.setItem(machinePrefsKey(machineId), JSON.stringify(prefs));
}
export const VIEW_OPTS = 'cctui_view_opts';
export const LIST_DENSITY = 'cctui_list_density';
// Main session list layout: 'list' (rows, default) or 'card' (responsive grid
// of detailed cards) — CCT-297 item 16.
export const LIST_VIEW = 'cctui_list_view';
// Whether the "Dispatched" session group is collapsed/hidden (CCT-279 item 6).
export const DISPATCHED_COLLAPSED = 'cctui_dispatched_collapsed';
