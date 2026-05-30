import { browser } from '$app/environment';

const KEY = 'cctui_theme';
type Mode = 'dark' | 'light';

class Theme {
	current = $state<Mode>('dark');

	constructor() {
		if (browser) {
			const saved = localStorage.getItem(KEY) as Mode | null;
			this.current = saved ?? 'dark';
			this.apply();
		}
	}
	private apply() {
		if (browser) document.documentElement.setAttribute('data-theme', this.current);
	}
	toggle() {
		this.current = this.current === 'dark' ? 'light' : 'dark';
		if (browser) localStorage.setItem(KEY, this.current);
		this.apply();
	}
}

export const theme = new Theme();
