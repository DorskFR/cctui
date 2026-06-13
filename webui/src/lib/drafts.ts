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
 * model + effort, account and working directory last used on a given machine,
 * so the next spawn on e.g. dev1 re-selects what you usually run there.
 * Keyed by machine id. */
export interface MachineSpawnPrefs {
	adapter_id: string;
	model_claude: string;
	model_codex: string;
	effort_claude: string;
	effort_codex: string;
	account: string;
	working_dir: string;
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

/** The session name last submitted from the spawn dialog (either target).
 * A fresh dialog open proposes it with a bumped numeric suffix. */
export const LAST_SPAWN_NAME = 'cctui_last_spawn_name';

/** Label ids (comma-joined) last attached from the spawn dialog (CCT-360). A
 * fresh dialog open defaults its label picker to this set; an empty submit
 * clears it. */
export const LAST_SPAWN_LABELS = 'cctui_last_spawn_labels';

/** Next proposed session name: bump a trailing `-<n>` suffix, else append
 * `-2` (`toto` → `toto-2`, `toto-5` → `toto-6`). Zero-padding is kept
 * (`run-09` → `run-10`). */
export function nextSessionName(last: string): string {
	const m = last.match(/^(.*)-(\d+)$/);
	if (!m) return `${last}-2`;
	const next = String(Number(m[2]) + 1);
	return `${m[1]}-${next.padStart(m[2].length, '0')}`;
}
export const VIEW_OPTS = 'cctui_view_opts';
export const LIST_DENSITY = 'cctui_list_density';
// Main session list layout: 'list' (rows, default) or 'card' (responsive grid
// of detailed cards) — CCT-297 item 16.
export const LIST_VIEW = 'cctui_list_view';
// Which session section is in view (CCT-322): 'starred' | 'live' | 'dispatched'
// | 'archived'. Replaces the old archived on/off checkbox with a 4-way picker.
export const LIST_SECTION = 'cctui_list_section';
// Selected label-filter ids (CCT-360), comma-joined. Empty = show all.
export const LIST_LABELS = 'cctui_list_labels';
