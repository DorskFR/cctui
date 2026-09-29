// The kit owns the theme registry, the store and the picker; the app only
// re-exports them so every consumer reads one instance.
import { browser } from '$app/environment';
import { preferenceFrom, theme, type ThemePreference } from '@dorsk/tsumikit';

export { theme, THEMES } from '@dorsk/tsumikit';

const LEGACY_KEY = 'cctui_theme_pref';

/** Adopt the preference cctui used to cache under its own key, then drop it. */
export function migrateLegacyThemePref() {
	if (!browser) return;
	let raw: string | null = null;
	try {
		raw = localStorage.getItem(LEGACY_KEY);
	} catch {
		return;
	}
	if (!raw) return;
	try {
		const parsed = JSON.parse(raw) as Partial<ThemePreference>;
		theme.hydrate(
			preferenceFrom(
				{ themeMode: parsed.mode, lightTheme: parsed.light, darkTheme: parsed.dark },
				theme.slotOf
			)
		);
		theme.choose(theme.choice);
	} catch {
		return;
	}
	try {
		localStorage.removeItem(LEGACY_KEY);
	} catch {
		return;
	}
}

migrateLegacyThemePref();
