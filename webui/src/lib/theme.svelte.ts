import { browser } from '$app/environment';

const KEY = 'cctui_theme';
export type Mode = 'dark' | 'light' | 'sepia';

// Cycle order for the header toggle: dark → light → sepia → dark.
const ORDER: Mode[] = ['dark', 'light', 'sepia'];
const ICONS: Record<Mode, string> = { dark: '☾', light: '☀', sepia: '✶' };

class Theme {
	current = $state<Mode>('dark');

	constructor() {
		if (browser) {
			const saved = localStorage.getItem(KEY) as Mode | null;
			this.current = saved && ORDER.includes(saved) ? saved : 'dark';
			this.apply();
		}
	}
	private apply() {
		if (browser) document.documentElement.setAttribute('data-theme', this.current);
	}
	/** Icon for the *next* theme you'd switch to (hints what the button does). */
	get icon(): string {
		return ICONS[this.current];
	}
	toggle() {
		const i = ORDER.indexOf(this.current);
		this.current = ORDER[(i + 1) % ORDER.length];
		if (browser) localStorage.setItem(KEY, this.current);
		this.apply();
	}
}

export const theme = new Theme();
