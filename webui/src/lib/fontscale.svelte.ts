import { browser } from '$app/environment';

// Global UI font scale (CCT-250 item 3). The whole design system is rem-based,
// so scaling the document root font-size scales ALL text — main UI, chat,
// markdown tables, badges — uniformly. Persisted and applied to <html>.
const KEY = 'cctui_font_scale';

// Multiplier of the 16px root. Range is wider than the old chat-only slider so
// the UI can go meaningfully larger; the slider widget size itself is unchanged.
export const SCALE_MIN = 0.8;
export const SCALE_MAX = 1.6;
const DEFAULT = 1;

function clamp(n: number): number {
	if (!Number.isFinite(n)) return DEFAULT;
	return Math.min(SCALE_MAX, Math.max(SCALE_MIN, n));
}

class FontScale {
	current = $state<number>(DEFAULT);

	constructor() {
		if (browser) {
			const saved = Number(localStorage.getItem(KEY));
			this.current = saved ? clamp(saved) : DEFAULT;
			this.apply();
		}
	}
	private apply() {
		if (!browser) return;
		// Root stays at 16px nominal; scale via font-size percentage so every
		// rem downstream tracks it.
		document.documentElement.style.fontSize = `${this.current * 100}%`;
	}
	set(value: number) {
		this.current = clamp(value);
		if (browser) localStorage.setItem(KEY, String(this.current));
		this.apply();
	}
}

export const fontScale = new FontScale();
