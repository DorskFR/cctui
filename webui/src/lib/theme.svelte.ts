import { browser } from '$app/environment';

const KEY = 'cctui_theme';

// Theme registry — the single list the picker and the store both read. Adding a
// theme = one entry here + one [data-theme="id"] block in variables.css. The
// `mode` field groups the theme into the picker's light/dark sections (TSU-1 /
// CCT-401).
export const THEMES = [
	// ── Light ── bright, paper-white surfaces
	{ id: 'light', label: 'Light', icon: '☀', themeColor: '#f6f7f9', mode: 'light' },
	{ id: 'highcontrast', label: 'High Contrast Light', icon: '◻', themeColor: '#ffffff', mode: 'light' },
	// ── Medium-light (TSU-1) ── easy on the eyes, not blinding, distinct bases
	{ id: 'gruvboxlight', label: 'Gruvbox Light', icon: '◇', themeColor: '#fbf1c7', mode: 'light' },
	{ id: 'solarizedlight', label: 'Solarized Light', icon: '◑', themeColor: '#fdf6e3', mode: 'light' },
	{ id: 'everforestlight', label: 'Everforest Light', icon: '✾', themeColor: '#fdf6e3', mode: 'light' },
	{ id: 'rosepinedawn', label: 'Rosé Pine Dawn', icon: '✿', themeColor: '#faf4ed', mode: 'light' },
	{ id: 'latte', label: 'Catppuccin Latte', icon: 'L', themeColor: '#eff1f5', mode: 'light' },
	{ id: 'nordlight', label: 'Nord Light', icon: 'n', themeColor: '#eceff4', mode: 'light' },
	{ id: 'tokyoday', label: 'Tokyo Night Day', icon: '✧', themeColor: '#e1e2e7', mode: 'light' },
	{ id: 'kanagawalotus', label: 'Kanagawa Lotus', icon: '❁', themeColor: '#f2ecbc', mode: 'light' },
	{ id: 'sepia', label: 'Sepia', icon: '✶', themeColor: '#f4ecd8', mode: 'light' },
	// ── Dark ──
	{ id: 'dark', label: 'Dark', icon: '☾', themeColor: '#0f1115', mode: 'dark' },
	{ id: 'colorblind', label: 'Color-blind safe', icon: '◐', themeColor: '#16181d', mode: 'dark' },
	{ id: 'mocha', label: 'Catppuccin Mocha', icon: 'M', themeColor: '#1e1e2e', mode: 'dark' },
	{ id: 'dracula', label: 'Dracula', icon: 'D', themeColor: '#282a36', mode: 'dark' },
	{ id: 'nord', label: 'Nord', icon: 'N', themeColor: '#2e3440', mode: 'dark' },
	{ id: 'tokyonight', label: 'Tokyo Night', icon: '✦', themeColor: '#1a1b26', mode: 'dark' },
	{ id: 'gruvbox', label: 'Gruvbox', icon: '◆', themeColor: '#282828', mode: 'dark' },
	{ id: 'solarized', label: 'Solarized Dark', icon: '◒', themeColor: '#002b36', mode: 'dark' },
	{ id: 'rosepine', label: 'Rosé Pine', icon: '❀', themeColor: '#191724', mode: 'dark' },
	{ id: 'onedark', label: 'One Dark', icon: '①', themeColor: '#282c34', mode: 'dark' },
	{ id: 'everforest', label: 'Everforest', icon: '☘', themeColor: '#2d353b', mode: 'dark' },
	{ id: 'monokai', label: 'Monokai', icon: '✸', themeColor: '#272822', mode: 'dark' },
	{ id: 'amoled', label: 'AMOLED (high contrast)', icon: '◼', themeColor: '#000000', mode: 'dark' }
] as const;

export type Mode = (typeof THEMES)[number]['id'];
export type ThemeMode = (typeof THEMES)[number]['mode'];
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
