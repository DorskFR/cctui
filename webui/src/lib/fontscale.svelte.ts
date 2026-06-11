import { browser } from '$app/environment';

// Global UI font scale (CCT-250 item 3, range fixed in CCT-305). The control
// multiplies ONLY the font-size tokens (--fs-scale, consumed by every --fs-* in
// variables.css) — NOT the document root font-size. Scaling the root grew every
// rem-based chrome dimension (button heights, padding, the badges) in lockstep,
// so bumping the size up ballooned the UI while text "barely grew" relative to
// it (the CCT-305 regression). Driving --fs-scale instead leaves spacing /
// control sizes / the px-pinned header fixed and enlarges only text, so the top
// of the range gives big readable chat/session text rather than a giant UI.
const KEY = 'cctui_font_scale';

// Discrete scale levels (CCT-297 #11). A continuous slider let the value change
// on every pixel of drag, reflowing the UI live ("UI seizure") and — since the
// header hosted the control — sliding the thumb out from under the cursor. Five
// fixed steps remove the per-pixel churn: one click = one stable relayout. Each
// level is a text-size multiplier (--fs-scale); the top end is deliberately
// large (1.5×) so "Largest" yields genuinely big, readable text (CCT-305).
export interface ScaleLevel {
	id: string;
	label: string;
	value: number;
}
export const SCALE_LEVELS: ScaleLevel[] = [
	{ id: 'smallest', label: 'Smallest', value: 0.85 },
	{ id: 'small', label: 'Small', value: 0.925 },
	{ id: 'normal', label: 'Normal', value: 1 },
	{ id: 'large', label: 'Large', value: 1.25 },
	{ id: 'largest', label: 'Largest', value: 1.5 }
];
const DEFAULT_LEVEL = 'normal';

// Backwards range used elsewhere; kept as the level bounds.
export const SCALE_MIN = SCALE_LEVELS[0].value;
export const SCALE_MAX = SCALE_LEVELS[SCALE_LEVELS.length - 1].value;

function levelById(id: string): ScaleLevel {
	return SCALE_LEVELS.find((l) => l.id === id) ?? SCALE_LEVELS[2];
}
// Map an arbitrary saved multiplier (older slider value) to the nearest level id.
function nearestLevel(value: number): string {
	if (!Number.isFinite(value)) return DEFAULT_LEVEL;
	let best = SCALE_LEVELS[2];
	for (const l of SCALE_LEVELS) {
		if (Math.abs(l.value - value) < Math.abs(best.value - value)) best = l;
	}
	return best.id;
}

class FontScale {
	levelId = $state<string>(DEFAULT_LEVEL);
	current = $derived(levelById(this.levelId).value);

	constructor() {
		if (browser) {
			const raw = localStorage.getItem(KEY);
			// The key historically stored a numeric multiplier; migrate it to the
			// nearest discrete level. New writes store the level id.
			if (raw) this.levelId = SCALE_LEVELS.some((l) => l.id === raw) ? raw : nearestLevel(Number(raw));
			this.apply();
		}
	}
	private apply() {
		if (!browser) return;
		// Drive the font-token scale only (CCT-305) — the root font-size stays at
		// its nominal 16px so rem-based chrome (spacing, control heights, header)
		// is untouched while every --fs-* token grows with the level.
		document.documentElement.style.setProperty('--fs-scale', String(levelById(this.levelId).value));
	}
	set(id: string) {
		this.levelId = SCALE_LEVELS.some((l) => l.id === id) ? id : DEFAULT_LEVEL;
		if (browser) localStorage.setItem(KEY, this.levelId);
		this.apply();
	}
}

export const fontScale = new FontScale();
