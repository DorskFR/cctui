// Session-label color model (CCT-360). Labels carry an HSL *hue* (0–360),
// reusing the same hue infrastructure as MachineBadge / the ColorPicker /
// Swatch atoms — no hardcoded hex. The hue is persisted in the label's opaque
// `color` string (the server never parses it); an unset/unparseable value falls
// back to a deterministic name hash, exactly like an "Auto" machine color.

import { hashHue } from '$lib/format';

/** Preset hues offered by the label ColorPicker, matching the per-machine
 *  palette (CCT-222/251). `null` (Auto) → deterministic name hash. */
export const LABEL_HUES = [0, 30, 60, 90, 120, 150, 180, 210, 240, 270, 300, 330];

/** The hue explicitly stored on a label, or `null` when unset ("Auto"). Used as
 *  the ColorPicker `value` so the Auto swatch reads as selected when unset. */
export function storedHue(color: string): number | null {
	const n = parseInt(color, 10);
	return Number.isFinite(n) ? ((n % 360) + 360) % 360 : null;
}

/** The hue to actually paint a label with: the stored hue, or a name hash when
 *  unset. */
export function labelHue(label: { name: string; color: string }): number {
	return storedHue(label.color) ?? hashHue(label.name);
}

/** Persisted `color` string for a chosen hue (`null` = Auto/name hash → ""). */
export function hueToColor(hue: number | null): string {
	return hue == null ? '' : String(hue);
}

/** Inline tint for a label Badge, mirroring MachineBadge (CCT-272): the theme
 *  supplies `<sat%> <light%>` pairs in the --mach-* tokens, and the per-label
 *  hue resolves against them in a real custom property. Applied inline so it
 *  crosses the Badge component boundary regardless of scope. */
export function labelTint(label: { name: string; color: string }): string {
	return (
		`--mh:${labelHue(label)};` +
		'background:hsl(var(--mh) var(--mach-bg-sl));' +
		'color:hsl(var(--mh) var(--mach-fg-sl));' +
		'border-color:hsl(var(--mh) var(--mach-border-sl))'
	);
}
