import { browser } from '$app/environment';

const KEY = 'cctui_theme';

// Theme registry — the single list the picker and the store both read. Adding
// a theme = one entry here + one [data-theme="id"] block in variables.css.
// The `mode` field groups the theme into the picker's light/dark sections.
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

// "Auto" is not a palette: it defers to the OS/browser via
// `prefers-color-scheme` and resolves to one of the two base themes below. It
// lives outside THEMES so the registry stays a list of real palettes (every
// entry there has a [data-theme] block in variables.css); the pickers prepend
// it themselves.
export const AUTO = { id: 'auto', label: 'Auto', icon: '\u25d1', mode: 'auto' } as const;
export const AUTO_DARK: ThemeId = 'dark';
export const AUTO_LIGHT: ThemeId = 'light';

export type ThemeId = (typeof THEMES)[number]['id'];
export type Mode = ThemeId | typeof AUTO.id;
export type ThemeMode = (typeof THEMES)[number]['mode'];
type ThemeOption = (typeof THEMES)[number];

// 'auto' leads the cycle so the header's keyboard/scroll cycling can reach it.
const ORDER: Mode[] = [AUTO.id, ...THEMES.map((theme) => theme.id)];

function isMode(value: string | null): value is Mode {
	return value === AUTO.id || THEMES.some((theme) => theme.id === value);
}

function optionFor(mode: ThemeId): ThemeOption {
	return THEMES.find((theme) => theme.id === mode) ?? THEMES[0];
}

// The media query that backs 'auto'. Read lazily: jsdom and older Safari can
// leave matchMedia undefined, and a missing query must degrade to light rather
// than throw at module load.
function prefersDark(): boolean {
	if (!browser || typeof window.matchMedia !== 'function') return false;
	return window.matchMedia('(prefers-color-scheme: dark)').matches;
}

class Theme {
	current = $state<Mode>('dark');
	// Bumped by the media-query listener so `resolved` (and everything derived
	// from it) re-runs when the OS flips light/dark while we sit on 'auto'.
	private systemDark = $state(false);

	constructor() {
		if (browser) {
			const saved = localStorage.getItem(KEY);
			this.current = isMode(saved) ? saved : 'dark';
			this.systemDark = prefersDark();
			this.watchSystem();
			this.apply();
		}
	}
	// Follow the OS preference for as long as the page lives. The listener stays
	// attached whatever the current theme: it only costs a state write, and it
	// means switching to 'auto' later is already up to date.
	private watchSystem() {
		if (!browser || typeof window.matchMedia !== 'function') return;
		const mq = window.matchMedia('(prefers-color-scheme: dark)');
		const onChange = (e: MediaQueryListEvent) => {
			this.systemDark = e.matches;
			if (this.current === AUTO.id) this.apply();
		};
		if (typeof mq.addEventListener === 'function') mq.addEventListener('change', onChange);
		else mq.addListener?.(onChange);
	}
	private apply() {
		if (!browser) return;
		document.documentElement.setAttribute('data-theme', this.resolved);
		document
			.querySelector<HTMLMetaElement>('meta[name="theme-color"]')
			?.setAttribute('content', this.option.themeColor);
	}
	// The palette actually painted: identity for a real theme, the OS choice for
	// 'auto'. `data-theme` and the theme-color meta must never see 'auto' — no
	// such block exists in variables.css.
	get resolved(): ThemeId {
		if (this.current !== AUTO.id) return this.current;
		return this.systemDark ? AUTO_DARK : AUTO_LIGHT;
	}
	get isAuto(): boolean {
		return this.current === AUTO.id;
	}
	get option(): ThemeOption {
		return optionFor(this.resolved);
	}
	// Label and icon describe the *selection*, not the resolution, so the header
	// tooltip reads "Auto" rather than the palette auto happens to land on.
	get label(): string {
		return this.isAuto ? AUTO.label : this.option.label;
	}
	get icon(): string {
		return this.isAuto ? AUTO.icon : this.option.icon;
	}
	get next(): Mode {
		const i = ORDER.indexOf(this.current);
		return ORDER[(i + 1) % ORDER.length];
	}
	toggle() {
		this.set(this.next);
	}
	set(mode: Mode) {
		this.current = mode;
		if (browser) localStorage.setItem(KEY, this.current);
		this.apply();
	}
}

export const theme = new Theme();
