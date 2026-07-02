import { browser } from '$app/environment';
import { api } from './api';
import { auth } from './auth.svelte';
import type { SettingsPayload } from '@bindings/SettingsPayload';

// Server-persisted, user-scoped app settings (CCT-426, epic CCT-357). The whole
// preference catalogue lives in a single JSON blob behind GET/PUT
// /api/v1/settings, mirrored into localStorage for instant paint + offline
// fallback. The `settings` singleton is the single source of truth for the
// webui; the legacy theme/fontScale/notify singletons remain the runtime
// drivers for those three and are simply MIRRORED into this blob (the Settings
// panel drives both).
const KEY = 'cctui_settings';

// Bumped when the persisted shape changes; `migrate()` walks an older payload up
// to this version. v1 is the initial schema (no migrations yet).
export const CURRENT_VERSION = 1;

// Debounce window for the PUT — coalesces a burst of toggles into one write.
const SAVE_DEBOUNCE_MS = 400;

export interface NewSessionSettings {
	/** ON: the last-used spawn config (per-machine prefs / draft) wins, with
	 *  these defaults as the first-use fallback. OFF: seed purely from defaults. */
	rememberLastUsed: boolean;
	defaultTarget: 'machine' | 'dispatch';
	defaultMachineId: string | null;
	defaultDispatcherId: string | null;
	defaultAdapter: string;
	defaultModelClaude: string;
	defaultModelCodex: string;
	defaultEffortClaude: string;
	defaultEffortCodex: string;
	defaultPermissionMode: string;
	defaultAccount: string | null;
	defaultLabels: string[];
}

export interface SessionListSettings {
	sort: 'activity' | 'created' | 'name';
	view: 'list' | 'card';
	density: 'compact' | 'normal';
	section: string;
	labelFilter: string[];
}

export interface DisplaySettings {
	theme: string;
	fontScale: number;
	// Cmd/Ctrl+E in an open conversation interrupts any in-flight turn and then
	// archives the session (Beeper/Slack-style archive chord). Preserved from the
	// previous localStorage-only Settings (CCT-426).
	archiveShortcut: boolean;
	notifyEnabled: boolean;
	notifySound: boolean;
}

// The claude-code execution harness modes (epic CCT-494). Stored top-level in the
// settings blob as `data.harnessMode` because the server reads it from there
// (see settings.rs::harness_mode_of) to drive per-machine Reconcile. Codex
// sessions ignore this. An unknown stored value is clamped to `bg` server-side.
export type HarnessMode = 'bg' | 'sdk' | 'oneshot';
export const HARNESS_MODES: readonly HarnessMode[] = ['bg', 'sdk', 'oneshot'];
export const DEFAULT_HARNESS_MODE: HarnessMode = 'bg';

/** Clamp an arbitrary stored value to a known harness mode (mirrors the server's
 *  clamp so an unknown/missing value renders as `bg`). */
export function clampHarnessMode(v: unknown): HarnessMode {
	return HARNESS_MODES.includes(v as HarnessMode) ? (v as HarnessMode) : DEFAULT_HARNESS_MODE;
}

export interface SettingsState {
	newSession: NewSessionSettings;
	sessionList: SessionListSettings;
	display: DisplaySettings;
	// Claude harness mode (epic CCT-494). Top-level so it serializes as
	// `data.harnessMode`, which the server reads to drive each daemon's Reconcile.
	harnessMode: HarnessMode;
	// Reserved for a future keyboard-shortcuts surface (no UI yet, CCT-426).
	shortcutsEnabled: boolean;
	keymap: Record<string, string>;
}

const DEFAULTS: SettingsState = {
	newSession: {
		rememberLastUsed: true,
		defaultTarget: 'machine',
		defaultMachineId: null,
		defaultDispatcherId: null,
		defaultAdapter: 'claude-code',
		defaultModelClaude: '',
		defaultModelCodex: '',
		defaultEffortClaude: '',
		defaultEffortCodex: '',
		// '' = unset: let the account default (else claude's own default) apply
		// rather than forcing a mode into every spawn (CCT-542).
		defaultPermissionMode: '',
		defaultAccount: null,
		defaultLabels: []
	},
	sessionList: {
		sort: 'activity',
		view: 'list',
		density: 'normal',
		section: '',
		labelFilter: []
	},
	display: {
		theme: 'dark',
		fontScale: 1,
		archiveShortcut: true,
		notifyEnabled: false,
		notifySound: true
	},
	harnessMode: DEFAULT_HARNESS_MODE,
	shortcutsEnabled: false,
	keymap: {}
};

// Deep-merge a partial saved blob over DEFAULTS so a value missing from an older
// payload (a field added in a later release) falls back to its default rather
// than becoming undefined. One level of nesting covers the catalogue shape.
function mergeDefaults(partial: Partial<SettingsState> | null | undefined): SettingsState {
	const p = partial ?? {};
	return {
		newSession: { ...DEFAULTS.newSession, ...(p.newSession ?? {}) },
		sessionList: { ...DEFAULTS.sessionList, ...(p.sessionList ?? {}) },
		display: { ...DEFAULTS.display, ...(p.display ?? {}) },
		// Clamp to a known mode so an unknown stored value renders as `bg` (matches
		// the server's clamp on PUT).
		harnessMode: clampHarnessMode(p.harnessMode),
		shortcutsEnabled: p.shortcutsEnabled ?? DEFAULTS.shortcutsEnabled,
		keymap: p.keymap ?? DEFAULTS.keymap
	};
}

// Client-side payload migration chain, mirroring the server's idea: walk an
// older `data` blob up to CURRENT_VERSION. v1 is a passthrough — add a `case`
// per version bump. Pure; never throws.
function migrate(data: unknown, version: number): Partial<SettingsState> {
	let d = (data ?? {}) as Partial<SettingsState>;
	let v = version;
	// while (v < CURRENT_VERSION) { switch (v) { case 1: d = …; v = 2; break; } }
	void v;
	return d;
}

class Settings {
	state = $state<SettingsState>(mergeDefaults(null));

	private saveTimer: ReturnType<typeof setTimeout> | null = null;
	private loaded = false;

	constructor() {
		if (browser) {
			// Synchronous seed from the localStorage cache for instant paint (also the
			// offline fallback). Tolerate corrupt/blocked storage — never throw during
			// module init (which would blank the whole UI).
			try {
				const raw = localStorage.getItem(KEY);
				if (raw) this.state = mergeDefaults(JSON.parse(raw) as Partial<SettingsState>);
			} catch {
				this.state = mergeDefaults(null);
			}
		}
	}

	/** Pull the server copy once auth is known, run the migration chain, merge
	 *  over defaults, and refresh the cache. Tolerates failure (401/offline) by
	 *  keeping the cached/default state. Safe to call repeatedly; runs once. */
	async load(): Promise<void> {
		if (!browser || this.loaded || !auth.isAuthed) return;
		this.loaded = true;
		try {
			const payload = await api.get<SettingsPayload>('/settings');
			const migrated = migrate(payload.data, payload.version ?? CURRENT_VERSION);
			this.state = mergeDefaults(migrated);
			this.writeCache();
		} catch {
			// 401 / offline / decode error — keep the cached or default state.
		}
	}

	private writeCache() {
		if (browser) {
			try {
				localStorage.setItem(KEY, JSON.stringify(this.state));
			} catch {
				/* quota / blocked storage — the in-memory state still holds it */
			}
		}
	}

	private scheduleSave() {
		if (!browser || !auth.isAuthed) return;
		if (this.saveTimer) clearTimeout(this.saveTimer);
		this.saveTimer = setTimeout(() => {
			this.saveTimer = null;
			const body: SettingsPayload = {
				version: CURRENT_VERSION,
				data: this.state as unknown as SettingsPayload['data']
			};
			// Fire-and-forget; the cache already holds the value if the PUT drops.
			void api.put('/settings', body).catch(() => {});
		}, SAVE_DEBOUNCE_MS);
	}

	/** Persist after a mutation: cache immediately, debounce the server PUT. */
	private persist() {
		this.writeCache();
		this.scheduleSave();
	}

	// Section setters — replace a whole group (or a subset of its fields) and
	// persist. Components mutate via these so every write goes through the cache +
	// debounced save path.
	setNewSession(patch: Partial<NewSessionSettings>) {
		this.state.newSession = { ...this.state.newSession, ...patch };
		this.persist();
	}
	setSessionList(patch: Partial<SessionListSettings>) {
		this.state.sessionList = { ...this.state.sessionList, ...patch };
		this.persist();
	}
	setDisplay(patch: Partial<DisplaySettings>) {
		this.state.display = { ...this.state.display, ...patch };
		this.persist();
	}

	// Claude harness mode (epic CCT-494). Persisted top-level so it serializes as
	// `data.harnessMode`; the server clamps unknown values on PUT and pushes a
	// fresh Reconcile to the user's connected daemons within ~1s.
	setHarnessMode(mode: HarnessMode) {
		this.state.harnessMode = clampHarnessMode(mode);
		this.persist();
	}

	get harnessMode(): HarnessMode {
		return clampHarnessMode(this.state.harnessMode);
	}

	toggleArchiveShortcut() {
		this.setDisplay({ archiveShortcut: !this.state.display.archiveShortcut });
	}

	// Convenience reader for the most-used toggle (keeps call sites terse).
	get archiveShortcut(): boolean {
		return this.state.display.archiveShortcut;
	}
}

export const settings = new Settings();
