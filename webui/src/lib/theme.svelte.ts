import { browser } from '$app/environment';

const KEY = 'cctui_theme';

export const THEMES = [
	{ id: 'dark', label: 'Dark', icon: '☾', themeColor: '#0f1115' },
	{ id: 'light', label: 'Light', icon: '☀', themeColor: '#f6f7f9' },
	{ id: 'sepia', label: 'Sepia', icon: '✶', themeColor: '#f4ecd8' },
	{ id: 'mocha', label: 'Catppuccin Mocha', icon: 'M', themeColor: '#1e1e2e' },
	{ id: 'dracula', label: 'Dracula', icon: 'D', themeColor: '#282a36' },
	{ id: 'nord', label: 'Nord', icon: 'N', themeColor: '#2e3440' },
	{ id: 'tokyonight', label: 'Tokyo Night', icon: '✦', themeColor: '#1a1b26' },
	{ id: 'gruvbox', label: 'Gruvbox', icon: '◆', themeColor: '#282828' },
	{ id: 'solarized', label: 'Solarized Dark', icon: '◐', themeColor: '#002b36' },
	{ id: 'rosepine', label: 'Rosé Pine', icon: '❀', themeColor: '#191724' },
	{ id: 'onedark', label: 'One Dark', icon: '①', themeColor: '#282c34' },
	{ id: 'everforest', label: 'Everforest', icon: '☘', themeColor: '#2d353b' },
	{ id: 'monokai', label: 'Monokai', icon: '✸', themeColor: '#272822' },
	{ id: 'amoled', label: 'AMOLED (high contrast)', icon: '◼', themeColor: '#000000' },
	{ id: 'highcontrast', label: 'High Contrast Light', icon: '◻', themeColor: '#ffffff' }
] as const;

export type Mode = (typeof THEMES)[number]['id'];
type ThemeOption = (typeof THEMES)[number];

const ORDER: Mode[] = THEMES.map((theme) => theme.id);

function isMode(value: string | null): value is Mode {
	return THEMES.some((theme) => theme.id === value);
}

function optionFor(mode: Mode): ThemeOption {
	return THEMES.find((theme) => theme.id === mode) ?? THEMES[0];
}

class Theme {
	current = $state<Mode>('dark');

	constructor() {
		if (browser) {
			const saved = localStorage.getItem(KEY);
			this.current = isMode(saved) ? saved : 'dark';
			this.apply();
		}
	}
	private apply() {
		if (!browser) return;
		document.documentElement.setAttribute('data-theme', this.current);
		document
			.querySelector<HTMLMetaElement>('meta[name="theme-color"]')
			?.setAttribute('content', this.option.themeColor);
	}
	get option(): ThemeOption {
		return optionFor(this.current);
	}
	get label(): string {
		return this.option.label;
	}
	get icon(): string {
		return this.option.icon;
	}
	get next(): ThemeOption {
		const i = ORDER.indexOf(this.current);
		return optionFor(ORDER[(i + 1) % ORDER.length]);
	}
	toggle() {
		this.set(this.next.id);
	}
	set(mode: Mode) {
		this.current = mode;
		if (browser) localStorage.setItem(KEY, this.current);
		this.apply();
	}
}

export const theme = new Theme();
