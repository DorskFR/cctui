// Session-label helpers (CCT-360): a preset palette for the color picker and a
// luminance-based text-color pick so a chip's text stays readable on any hue.

/** Curated preset swatches offered first in the label color picker. A custom
 *  hex can still be chosen via the native color input next to them. */
export const LABEL_COLORS = [
	'#e11d48', // rose
	'#f97316', // orange
	'#eab308', // amber
	'#22c55e', // green
	'#14b8a6', // teal
	'#3b82f6', // blue
	'#6366f1', // indigo
	'#a855f7', // purple
	'#ec4899', // pink
	'#64748b' // slate
];

/** Black or white text for a given `#rrggbb` background, chosen by perceived
 *  luminance (W3C relative-luminance approximation) so chip text stays legible. */
export function labelTextColor(hex: string): string {
	const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
	if (!m) return '#ffffff';
	const n = parseInt(m[1], 16);
	const r = (n >> 16) & 0xff;
	const g = (n >> 8) & 0xff;
	const b = n & 0xff;
	// Perceived luminance (sRGB-weighted). >0.6 → light background → dark text.
	const lum = (0.299 * r + 0.587 * g + 0.114 * b) / 255;
	return lum > 0.6 ? '#1a1a1a' : '#ffffff';
}
