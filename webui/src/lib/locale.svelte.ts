import { browser } from '$app/environment';
import {
	baseLocale,
	getLocale,
	isLocale,
	locales,
	overwriteGetLocale,
	setLocale as paraglideSetLocale,
	type Locale
} from './paraglide/runtime';

export type { Locale };
export const LOCALES = locales;

// Human labels for the language picker. Not message keys: a language is always
// named in its own tongue, never translated.
export const LOCALE_LABELS: Record<Locale, string> = {
	en: 'English',
	fr: 'Français'
};

export function clampLocale(v: unknown): Locale | null {
	return typeof v === 'string' && isLocale(v) ? (v as Locale) : null;
}

// Runtime driver for the active UI language, mirroring theme.svelte.ts.
// getLocale() is overwritten to read the reactive `current` field so every
// `m.xxx()` call inside a Svelte template re-runs when the language flips — a
// live switch with no page reload. Paraglide still owns persistence via its
// localStorage strategy (setLocale writes it); the settings store layers the
// server-persisted preference on top.
class LocaleStore {
	current = $state<Locale>(baseLocale);

	constructor() {
		if (browser) {
			try {
				this.current = getLocale();
			} catch {
				this.current = baseLocale;
			}
			overwriteGetLocale(() => this.current);
			document.documentElement.lang = this.current;
		}
	}

	set(locale: Locale) {
		if (!isLocale(locale)) return;
		this.current = locale;
		paraglideSetLocale(locale, { reload: false });
		if (browser) document.documentElement.lang = locale;
	}
}

export const locale = new LocaleStore();
