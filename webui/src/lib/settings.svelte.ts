import { browser } from '$app/environment';
import { api } from './api';
import { auth } from './auth.svelte';
import { clampLocale, locale as localeStore, type Locale } from './locale.svelte';
import { theme, type Mode } from './theme.svelte';
import { fontScale } from './fontscale.svelte';
import { notify } from './notify.svelte';
import type { SettingsPayload } from '@bindings/SettingsPayload';
import {
	latestDirFor,
	latestEntryFor,
	putSpawnMemory,
	type SpawnMemoryEntry,
	type SpawnMemoryMap
} from './spawnMemory';

// Server-persisted, user-scoped app settings. The whole preference catalogue
// lives in a single JSON blob behind GET/PUT
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

export interface SessionListSettings {
	sort: 'activity' | 'created' | 'name';
	view: 'list' | 'card';
	density: 'compact' | 'normal';
	section: string;
	labelFilter: string[];
	// Card accent color and section grouping, sharing one
	// dimension enum (`Dimension` in sessions.logic.ts); both default 'none'.
	colorBy: 'none' | 'label' | 'working_dir' | 'machine';
	groupBy: 'none' | 'label' | 'working_dir' | 'machine';
	// How wide the centered session-list column is allowed to grow. Only bites on
	// screens wider than the chosen cap, so it is a desktop-only knob in practice:
	// a phone viewport is already narrower than the default.
	width: SessionListWidth;
	// Show the account NAME next to each row instead of the key glyph. Off by
	// default (the glyph keeps the row terse); worth turning on when several
	// accounts of the same provider are in play, where every glyph looks alike.
	accountNames: boolean;
}

// Session-list column widths, as the `size` handed to the layout Container.
// `default` keeps --content-max (the width every other screen uses).
export const SESSION_LIST_WIDTHS = ['default', 'wide', 'ultra', 'full'] as const;
export type SessionListWidth = (typeof SESSION_LIST_WIDTHS)[number];
export const DEFAULT_SESSION_LIST_WIDTH: SessionListWidth = 'default';

/** The CSS length for a width choice, or `undefined` to keep --content-max. */
export function sessionListWidthSize(w: SessionListWidth): string | undefined {
	switch (w) {
		case 'wide':
			return '72rem';
		case 'ultra':
			return 'var(--content-wide)';
		case 'full':
			return '100%';
		default:
			return undefined;
	}
}

/** Clamp an arbitrary stored value to a known width (an older/corrupt blob must
 *  not leak an invalid length into the Container's inline style). */
export function clampSessionListWidth(v: unknown): SessionListWidth {
	return SESSION_LIST_WIDTHS.includes(v as SessionListWidth)
		? (v as SessionListWidth)
		: DEFAULT_SESSION_LIST_WIDTH;
}

// Docked spawn panel: instead of the "+ New" button opening a modal, the whole
// new-session form stays pinned to one edge of the Sessions screen so a serial
// spawner never opens a dialog. Off by default. Desktop-only in practice: a
// narrow viewport falls back to the modal whatever is stored here.
export const SPAWN_DOCK_SIDES = ['left', 'right'] as const;
export type SpawnDockSide = (typeof SPAWN_DOCK_SIDES)[number];
export const DEFAULT_SPAWN_DOCK_SIDE: SpawnDockSide = 'right';

export interface SpawnDockSettings {
	enabled: boolean;
	side: SpawnDockSide;
}

/** Clamp a stored side to a known one (an older/corrupt blob must not pin the
 *  panel nowhere). */
export function clampSpawnDockSide(v: unknown): SpawnDockSide {
	return SPAWN_DOCK_SIDES.includes(v as SpawnDockSide)
		? (v as SpawnDockSide)
		: DEFAULT_SPAWN_DOCK_SIDE;
}

export interface DisplaySettings {
	theme: string;
	fontScale: number;
	// Cmd/Ctrl+E in an open conversation interrupts any in-flight turn and then
	// archives the session (Beeper/Slack-style archive chord). Preserved from the
	// previous localStorage-only Settings.
	archiveShortcut: boolean;
	notifyEnabled: boolean;
	notifySound: boolean;
}

// The claude-code execution harness modes. Stored top-level in the settings
// blob as `data.harnessMode` because the server reads it from there
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

// Whip-mode stall-phrase override. Stored top-level as
// `data.whipStopPhrases` because the server clamps it there and serves it to the
// whip Stop hook. `extend` appends to the daemon's compiled defaults; `replace`
// swaps them out. Empty phrases + extend + no guidance is a no-op the server drops.
export type WhipMode = 'extend' | 'replace';
export const WHIP_MODES: readonly WhipMode[] = ['extend', 'replace'];
export const DEFAULT_WHIP_MODE: WhipMode = 'extend';

export interface WhipStopPhrases {
	mode: WhipMode;
	phrases: string[];
	guidance: string;
}

/** Clamp a stored value to a known whip mode (mirrors the server's clamp). */
export function clampWhipMode(v: unknown): WhipMode {
	return WHIP_MODES.includes(v as WhipMode) ? (v as WhipMode) : DEFAULT_WHIP_MODE;
}

// Daemon-side secret redaction. `secretScrubEnabled` toggles live
// scrubbing; `secretScrubPatterns` are extra user regexes layered on the daemon's
// compiled defaults. Stored top-level as `data.secretScrubEnabled` /
// `data.secretScrubPatterns`, which the server clamps (validates each regex,
// caps count/length) and syncs to the daemon via Reconcile.
export interface SecretScrubPattern {
	name: string;
	regex: string;
	enabled: boolean;
}

export interface SettingsState {
	sessionList: SessionListSettings;
	display: DisplaySettings;
	// Docked spawn panel (Sessions screen). Top-level so it serializes as
	// `data.spawnDock`; the server passes it through untouched.
	spawnDock: SpawnDockSettings;
	// Claude harness mode. Top-level so it serializes as `data.harnessMode`,
	// which the server reads to drive each daemon's Reconcile.
	harnessMode: HarnessMode;
	// Whip-mode stall-phrase override. Top-level so it serializes as
	// `data.whipStopPhrases`, which the server clamps and feeds to the whip hook.
	whipStopPhrases: WhipStopPhrases;
	// Daemon-side secret redaction. Top-level so they serialize as
	// `data.secretScrubEnabled` / `data.secretScrubPatterns`, which the server
	// clamps and syncs to the daemon via Reconcile.
	secretScrubEnabled: boolean;
	secretScrubPatterns: SecretScrubPattern[];
	// Prefix auto-generated session names with an emoji. Top-level so it
	// serializes as `data.sessionEmojiPrefix`, which the server reads while it
	// persists the name the agent reported (cctui does not generate the name
	// itself). Off by default; a name the user typed is never decorated.
	sessionEmojiPrefix: boolean;
	// Per-(machine, working-dir) spawn memory: the config last
	// submitted from the spawn modal, keyed by machineMemoryKey/dispatchMemoryKey
	// (spawnMemory.ts), LRU-capped. Replaces the localStorage per-machine prefs
	// so the memory follows the user across browsers.
	spawnMemory: SpawnMemoryMap;
	// Reserved for a future keyboard-shortcuts surface (no UI yet).
	shortcutsEnabled: boolean;
	keymap: Record<string, string>;
	// UI language. Top-level so it serializes as `data.locale`, which
	// the server clamps to en|fr|null. `null` means "auto" — fall back to the
	// browser's language / the base locale (Paraglide resolves it at runtime).
	locale: Locale | null;
}

const DEFAULTS: SettingsState = {
	sessionList: {
		sort: 'activity',
		view: 'list',
		density: 'normal',
		section: '',
		labelFilter: [],
		colorBy: 'none',
		groupBy: 'none',
		width: DEFAULT_SESSION_LIST_WIDTH,
		accountNames: false
	},
	display: {
		theme: 'dark',
		fontScale: 1,
		archiveShortcut: true,
		notifyEnabled: false,
		notifySound: true
	},
	spawnDock: { enabled: false, side: DEFAULT_SPAWN_DOCK_SIDE },
	harnessMode: DEFAULT_HARNESS_MODE,
	whipStopPhrases: { mode: DEFAULT_WHIP_MODE, phrases: [], guidance: '' },
	secretScrubEnabled: false,
	secretScrubPatterns: [],
	sessionEmojiPrefix: false,
	spawnMemory: {},
	shortcutsEnabled: false,
	keymap: {},
	locale: null
};

// Deep-merge a partial saved blob over DEFAULTS so a value missing from an older
// payload (a field added in a later release) falls back to its default rather
// than becoming undefined. One level of nesting covers the catalogue shape.
// Stale keys in an older blob (e.g. a retired setting) are simply not copied
// over, and get pruned on the next save.
export function mergeDefaults(partial: Partial<SettingsState> | null | undefined): SettingsState {
	const p = partial ?? {};
	return {
		sessionList: {
			...DEFAULTS.sessionList,
			...(p.sessionList ?? {}),
			// Clamp so a stale/unknown stored value renders as the default column
			// width rather than an invalid CSS length.
			width: clampSessionListWidth(p.sessionList?.width),
			accountNames: p.sessionList?.accountNames === true
		},
		display: { ...DEFAULTS.display, ...(p.display ?? {}) },
		spawnDock: {
			enabled: p.spawnDock?.enabled === true,
			side: clampSpawnDockSide(p.spawnDock?.side)
		},
		// Clamp to a known mode so an unknown stored value renders as `bg` (matches
		// the server's clamp on PUT).
		harnessMode: clampHarnessMode(p.harnessMode),
		whipStopPhrases: mergeWhipStopPhrases(p.whipStopPhrases),
		secretScrubEnabled: p.secretScrubEnabled === true,
		secretScrubPatterns: mergeSecretScrubPatterns(p.secretScrubPatterns),
		sessionEmojiPrefix: p.sessionEmojiPrefix === true,
		spawnMemory: p.spawnMemory ?? {},
		shortcutsEnabled: p.shortcutsEnabled ?? DEFAULTS.shortcutsEnabled,
		keymap: p.keymap ?? DEFAULTS.keymap,
		locale: clampLocale(p.locale)
	};
}

// Coerce a stored whipStopPhrases value into the UI shape: clamp mode,
// keep only string phrases, coerce guidance to a string. The server drops a
// default block, so an absent value renders as the default.
function mergeWhipStopPhrases(v: Partial<WhipStopPhrases> | undefined): WhipStopPhrases {
	const raw = (v ?? {}) as Partial<WhipStopPhrases>;
	return {
		mode: clampWhipMode(raw.mode),
		phrases: Array.isArray(raw.phrases) ? raw.phrases.filter((p) => typeof p === 'string') : [],
		guidance: typeof raw.guidance === 'string' ? raw.guidance : ''
	};
}

// Coerce a stored secretScrubPatterns value into the UI shape: keep
// only well-formed `{ name, regex, enabled }` entries with a non-empty regex.
function mergeSecretScrubPatterns(v: unknown): SecretScrubPattern[] {
	if (!Array.isArray(v)) return [];
	return v
		.filter((e): e is Record<string, unknown> => !!e && typeof e === 'object')
		.map((e) => ({
			name: typeof e.name === 'string' ? e.name : '',
			regex: typeof e.regex === 'string' ? e.regex : '',
			enabled: e.enabled !== false
		}))
		.filter((e) => e.regex.trim().length > 0);
}

// Client-side payload migration chain, mirroring the server's idea: walk an
// older `data` blob up to CURRENT_VERSION. v1 is a passthrough — add a `case`
// per version bump. Pure; never throws.
function migrate(data: unknown, version: number): Partial<SettingsState> {
	const d = (data ?? {}) as Partial<SettingsState>;
	const v = version;
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
			this.applyDisplay();
			if (this.state.locale) localeStore.set(this.state.locale);
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
	setSessionList(patch: Partial<SessionListSettings>) {
		this.state.sessionList = { ...this.state.sessionList, ...patch };
		this.persist();
	}

	/** Column width of the session list, clamped on read so a blob written by a
	 *  newer/older build can never feed a bogus length to the Container. */
	get sessionListWidth(): SessionListWidth {
		return clampSessionListWidth(this.state.sessionList.width);
	}

	/** Whether session rows spell the account name out instead of the key glyph. */
	get accountNames(): boolean {
		return this.state.sessionList.accountNames === true;
	}
	// Docked spawn panel: on/off and which edge it pins to.
	setSpawnDock(patch: Partial<SpawnDockSettings>) {
		this.state.spawnDock = { ...this.state.spawnDock, ...patch };
		this.persist();
	}

	get spawnDock(): SpawnDockSettings {
		return {
			enabled: this.state.spawnDock.enabled === true,
			side: clampSpawnDockSide(this.state.spawnDock.side)
		};
	}

	setDisplay(patch: Partial<DisplaySettings>) {
		this.state.display = { ...this.state.display, ...patch };
		this.persist();
	}

	// Display drivers routed through the blob so theme/font/notify round-trip
	// across devices: every surface (header + settings panel) mutates the runtime
	// singleton AND records the value here, and `load()` replays the blob back
	// into the singletons via `applyDisplay`.
	setTheme(id: string) {
		theme.set(id as Mode);
		this.setDisplay({ theme: id });
	}

	setFontScaleLevel(levelId: string) {
		fontScale.set(levelId);
		this.setDisplay({ fontScale: fontScale.current });
	}

	setNotifySound(on: boolean) {
		notify.setSound(on);
		this.setDisplay({ notifySound: notify.sound });
	}

	/** Record the notifier's current enabled state after a header/panel toggle
	 *  (whose async permission prompt the caller owns). */
	recordNotifyEnabled() {
		this.setDisplay({ notifyEnabled: notify.enabled });
	}

	private applyDisplay() {
		const d = this.state.display;
		theme.set(d.theme as Mode);
		fontScale.setScale(d.fontScale);
		notify.applyPersisted(d.notifyEnabled, d.notifySound);
	}

	// Claude harness mode. Persisted top-level so it serializes as
	// `data.harnessMode`; the server clamps unknown values on PUT and pushes a
	// fresh Reconcile to the user's connected daemons within ~1s.
	setHarnessMode(mode: HarnessMode) {
		this.state.harnessMode = clampHarnessMode(mode);
		this.persist();
	}

	get harnessMode(): HarnessMode {
		return clampHarnessMode(this.state.harnessMode);
	}

	// Whip stall-phrase override. Persisted top-level; the server clamps
	// (trim/lowercase/dedupe/cap) and serves it to the whip Stop hook on next spawn.
	setWhipStopPhrases(patch: Partial<WhipStopPhrases>) {
		this.state.whipStopPhrases = { ...this.state.whipStopPhrases, ...patch };
		this.persist();
	}

	get whipStopPhrases(): WhipStopPhrases {
		return this.state.whipStopPhrases;
	}

	// Secret redaction. The server validates each regex on PUT and syncs
	// the effective list to the daemon via Reconcile.
	setSecretScrubEnabled(on: boolean) {
		this.state.secretScrubEnabled = on;
		this.persist();
	}

	setSecretScrubPatterns(patterns: SecretScrubPattern[]) {
		this.state.secretScrubPatterns = patterns;
		this.persist();
	}

	get secretScrubEnabled(): boolean {
		return this.state.secretScrubEnabled;
	}

	get secretScrubPatterns(): SecretScrubPattern[] {
		return this.state.secretScrubPatterns;
	}

	// Emoji prefix on agent-generated session names. Applied server-side when
	// the name lands, so it shows up everywhere the name does (list, cards,
	// notifications), not just in this browser.
	setSessionEmojiPrefix(on: boolean) {
		this.state.sessionEmojiPrefix = on;
		this.persist();
	}

	get sessionEmojiPrefix(): boolean {
		return this.state.sessionEmojiPrefix;
	}

	// Spawn memory: write on spawn submit, recall on machine/cwd (or
	// dispatcher/repo) change in the spawn modal. Keys come from spawnMemory.ts.
	rememberSpawn(key: string, entry: Omit<SpawnMemoryEntry, 'at'>) {
		this.state.spawnMemory = putSpawnMemory(this.state.spawnMemory, key, {
			...entry,
			at: Date.now()
		});
		this.persist();
	}

	recallSpawn(key: string): SpawnMemoryEntry | null {
		return this.state.spawnMemory[key] ?? null;
	}

	/** The working dir most recently spawned on `machineId`, to pre-fill the cwd
	 *  (which then keys the full recall). */
	lastDirFor(machineId: string): string | null {
		return latestDirFor(this.state.spawnMemory, machineId);
	}

	/** The machine's most recent entry regardless of dir, used when the picked
	 *  cwd has no memory of its own. */
	lastEntryFor(machineId: string): SpawnMemoryEntry | null {
		return latestEntryFor(this.state.spawnMemory, machineId);
	}

	// UI language. Drives the Paraglide runtime immediately and persists
	// top-level as `data.locale` (server clamps to en|fr|null). `null` = auto.
	setLocale(next: Locale | null) {
		this.state.locale = clampLocale(next);
		if (this.state.locale) localeStore.set(this.state.locale);
		this.persist();
	}

	get locale(): Locale | null {
		return this.state.locale;
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
