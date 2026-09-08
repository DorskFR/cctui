import { browser } from '$app/environment';
import { theme } from './theme.svelte';
import {
	chooseTheme,
	DEFAULT_THEME_PREFERENCE,
	pickerValue,
	preferenceFrom,
	resolveTheme,
	type SlotOf,
	type ThemePreference
} from './themeMode';

// Runtime driver for the light/dark/auto preference. It owns the system
// `prefers-color-scheme` listener and pushes the resolved id into the kit's
// `theme` store, which does the actual painting. The preference is cached in
// localStorage so the first paint after a reload already honours `auto`
// (the server blob arrives later and replays through `hydrate`).
const KEY = 'cctui_theme_pref';
const QUERY = '(prefers-color-scheme: dark)';

const slotOf: SlotOf = (id) => {
	const def = theme.all.find((t) => t.id === id);
	return def ? def.mode : null;
};

function readCache(): ThemePreference {
	if (!browser) return DEFAULT_THEME_PREFERENCE;
	try {
		const raw = localStorage.getItem(KEY);
		if (!raw) return preferenceFrom({ theme: localStorage.getItem('tsumikit-theme') }, slotOf);
		const parsed = JSON.parse(raw) as Partial<ThemePreference>;
		return preferenceFrom(
			{ themeMode: parsed.mode, lightTheme: parsed.light, darkTheme: parsed.dark },
			slotOf
		);
	} catch {
		return DEFAULT_THEME_PREFERENCE;
	}
}

class ThemeMode {
	pref = $state<ThemePreference>(DEFAULT_THEME_PREFERENCE);
	systemDark = $state(false);

	constructor() {
		if (!browser) return;
		const mq = typeof window.matchMedia === 'function' ? window.matchMedia(QUERY) : null;
		this.systemDark = mq?.matches ?? false;
		mq?.addEventListener?.('change', (e) => {
			this.systemDark = e.matches;
			if (this.pref.mode === 'auto') this.paint();
		});
		this.pref = readCache();
		this.paint();
	}

	/** Theme id currently painted for the preference. */
	get resolved(): string {
		return resolveTheme(this.pref, this.systemDark);
	}

	/** What the pickers show as selected: `auto` or the pinned theme id. */
	get value(): string {
		return pickerValue(this.pref);
	}

	get slotOf(): SlotOf {
		return slotOf;
	}

	/** Replay a persisted preference (server blob) without touching the memory
	 *  of the other slot. */
	hydrate(pref: ThemePreference) {
		this.pref = pref;
		this.paint(false);
	}

	/** A picker choice: `auto`, or a theme id. Returns the new preference so
	 *  the caller can persist it. */
	choose(choice: string): ThemePreference {
		this.pref = chooseTheme(this.pref, choice, slotOf);
		this.paint();
		return this.pref;
	}

	private paint(cache = true) {
		const id = this.resolved;
		if (theme.has(id) && theme.current !== id) theme.set(id);
		if (cache && browser) {
			try {
				localStorage.setItem(KEY, JSON.stringify(this.pref));
			} catch {
				// Private mode / quota: the server blob still carries it.
			}
		}
	}
}

export const themeMode = new ThemeMode();
