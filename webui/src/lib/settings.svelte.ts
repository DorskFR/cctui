import { browser } from '$app/environment';

// Global, client-side app preferences. Persisted as a single JSON blob in
// localStorage (one key, not one-per-toggle) so new switches can be added
// without minting a new storage key each time. These are device/operator
// preferences — not server state — mirroring the theme / fontScale singletons.
const KEY = 'cctui_settings';

// The shape persisted to localStorage. Every field is a simple boolean toggle
// surfaced on the Settings page; add a field here + a default + a SETTING_DEFS
// entry to introduce a new global toggle.
export interface SettingsState {
	// Cmd/Ctrl+E in an open conversation interrupts any in-flight turn and then
	// archives the session (Beeper/Slack-style archive chord). Disable to turn
	// the shortcut off entirely.
	archiveShortcut: boolean;
}

const DEFAULTS: SettingsState = {
	archiveShortcut: true
};

export interface SettingDef {
	key: keyof SettingsState;
	label: string;
	description: string;
}

// Ordered list driving the Settings page UI. Keep in sync with SettingsState /
// DEFAULTS — one entry per toggle, top to bottom.
export const SETTING_DEFS: SettingDef[] = [
	{
		key: 'archiveShortcut',
		label: 'Archive shortcut',
		description:
			'In an open conversation, ⌘ E (Mac) / Ctrl + E interrupts any running turn and archives the session.'
	}
];

class Settings {
	state = $state<SettingsState>({ ...DEFAULTS });

	constructor() {
		if (browser) {
			try {
				const raw = localStorage.getItem(KEY);
				if (raw) {
					const parsed = JSON.parse(raw) as Partial<SettingsState>;
					// Merge over defaults so a value the saved blob is missing (a toggle
					// added in a later release) falls back to its default instead of
					// becoming undefined.
					this.state = { ...DEFAULTS, ...parsed };
				}
			} catch {
				// Corrupt/blocked storage — fall back to defaults rather than throwing
				// during module init (which would blank the whole UI).
				this.state = { ...DEFAULTS };
			}
		}
	}

	// Convenience reader for the most-used toggle (keeps call sites terse).
	get archiveShortcut(): boolean {
		return this.state.archiveShortcut;
	}

	set(key: keyof SettingsState, value: boolean) {
		this.state[key] = value;
		if (browser) localStorage.setItem(KEY, JSON.stringify(this.state));
	}

	toggle(key: keyof SettingsState) {
		this.set(key, !this.state[key]);
	}
}

export const settings = new Settings();
