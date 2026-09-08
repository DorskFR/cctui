// Pure logic behind the light/dark/auto theme preference. The kit's `theme`
// store only knows ONE active theme id; this layer remembers the last light and
// the last dark theme the user picked, and an `auto` mode that follows the
// system's `prefers-color-scheme`. Kept free of Svelte runes so it is unit-
// tested in isolation and shared by the header picker and Settings › Appearance.

export type ThemeSlot = 'light' | 'dark';
export type ThemeChoice = ThemeSlot | 'auto';

export interface ThemePreference {
	/** `auto` follows the system; `light` / `dark` pin the matching slot. */
	mode: ThemeChoice;
	/** Last light theme the user picked. */
	light: string;
	/** Last dark theme the user picked. */
	dark: string;
}

/** The picker value that means "follow the system". Never a theme id. */
export const AUTO_THEME = 'auto';

export const DEFAULT_THEME_PREFERENCE: ThemePreference = {
	mode: 'dark',
	light: 'light',
	dark: 'dark'
};

/** Which slot a theme id belongs to; `null` when the id is unknown. */
export type SlotOf = (id: string) => ThemeSlot | null;

/** The theme id to paint for a preference, given the system's current scheme. */
export function resolveTheme(pref: ThemePreference, systemDark: boolean): string {
	const slot: ThemeSlot = pref.mode === 'auto' ? (systemDark ? 'dark' : 'light') : pref.mode;
	return slot === 'dark' ? pref.dark : pref.light;
}

/** Apply a picker choice: `auto` switches mode only; a theme id pins its slot
 *  AND becomes that slot's remembered theme. Unknown ids leave the preference
 *  untouched so a stale blob can never paint an unregistered theme. */
export function chooseTheme(pref: ThemePreference, choice: string, slotOf: SlotOf): ThemePreference {
	if (choice === AUTO_THEME) return { ...pref, mode: 'auto' };
	const slot = slotOf(choice);
	if (!slot) return pref;
	return { ...pref, mode: slot, [slot]: choice };
}

/** Rebuild a preference from persisted fields. Older blobs only carried a
 *  single `theme`: it seeds whichever slot it belongs to and pins that mode, so
 *  nothing changes for a user who never touched the new picker. Unknown ids
 *  fall back to the defaults rather than being trusted. */
export function preferenceFrom(
	raw: { theme?: string | null; themeMode?: string | null; lightTheme?: string | null; darkTheme?: string | null },
	slotOf: SlotOf
): ThemePreference {
	const light = raw.lightTheme && slotOf(raw.lightTheme) === 'light' ? raw.lightTheme : DEFAULT_THEME_PREFERENCE.light;
	const dark = raw.darkTheme && slotOf(raw.darkTheme) === 'dark' ? raw.darkTheme : DEFAULT_THEME_PREFERENCE.dark;
	const pref: ThemePreference = { mode: DEFAULT_THEME_PREFERENCE.mode, light, dark };
	if (raw.themeMode === 'auto' || raw.themeMode === 'light' || raw.themeMode === 'dark') {
		return { ...pref, mode: raw.themeMode };
	}
	// Legacy blob: a lone `theme` pins its own slot.
	if (raw.theme) return chooseTheme(pref, raw.theme, slotOf);
	return pref;
}

/** The picker's current value: `auto`, or the pinned slot's theme id. */
export function pickerValue(pref: ThemePreference): string {
	return pref.mode === 'auto' ? AUTO_THEME : pref.mode === 'dark' ? pref.dark : pref.light;
}
