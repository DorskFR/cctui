/** Account identity glyph: the pure bits behind `AccountAvatar` — the fallback
 *  colour derived from the account id, the letter it carries, and the
 *  client-side mirror of the server's single-emoji rule. Pure and unit-tested so
 *  the same account looks identical on every screen. */

/** FNV-1a over the id: stable across reloads, machines and locales. */
function hash(id: string): number {
	let h = 0x811c9dc5;
	for (let i = 0; i < id.length; i++) {
		h ^= id.charCodeAt(i);
		h = Math.imul(h, 0x01000193) >>> 0;
	}
	return h;
}

export const accountHue = (id: string): number => hash(id) % 360;

const SAT = 55;
const LIGHT = 45;
/** Luminance at which white and black text have equal WCAG contrast. */
const WCAG_FLIP = 0.179;

function luminance(hue: number): number {
	const c = (1 - Math.abs((2 * LIGHT) / 100 - 1)) * (SAT / 100);
	const hp = hue / 60;
	const x = c * (1 - Math.abs((hp % 2) - 1));
	const m = LIGHT / 100 - c / 2;
	const [r, g, b] = (
		[
			[c, x, 0],
			[x, c, 0],
			[0, c, x],
			[0, x, c],
			[x, 0, c],
			[c, 0, x]
		] as const
	)[Math.min(5, Math.floor(hp))].map((v) => v + m);
	const lin = (v: number) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
	return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

/** The fallback square's colours for an account id: one mid-tone fill that
 *  reads on both light and dark grounds, with the text flipped to whichever of
 *  white/near-black contrasts with it. */
export function avatarColorsForHue(hue: number): { background: string; color: string } {
	return {
		background: `hsl(${hue} ${SAT}% ${LIGHT}%)`,
		color: luminance(hue) > WCAG_FLIP ? '#14161a' : '#ffffff'
	};
}

export const accountAvatarColors = (id: string): { background: string; color: string } =>
	avatarColorsForHue(accountHue(id));

/** First grapheme of the account name, upper-cased; `?` when it has none. */
export function accountInitial(name: string): string {
	const trimmed = (name ?? '').trim();
	if (!trimmed) return '?';
	const seg =
		typeof Intl !== 'undefined' && 'Segmenter' in Intl
			? [...new Intl.Segmenter().segment(trimmed)][0]?.segment
			: [...trimmed][0];
	return (seg ?? '?').toUpperCase();
}

const EMOJI_MAX_SCALARS = 8;
const isBase = (c: number) =>
	c === 0x203c ||
	c === 0x2049 ||
	c === 0x2122 ||
	c === 0x2139 ||
	(c >= 0x2190 && c <= 0x21ff) ||
	(c >= 0x2300 && c <= 0x23ff) ||
	(c >= 0x25aa && c <= 0x25ff) ||
	(c >= 0x2600 && c <= 0x27bf) ||
	c === 0x2934 ||
	c === 0x2935 ||
	(c >= 0x2b00 && c <= 0x2bff) ||
	c === 0x3030 ||
	c === 0x303d ||
	c === 0x3297 ||
	c === 0x3299 ||
	(c >= 0x1f000 && c <= 0x1faff);
const isModifier = (c: number) =>
	c === 0xfe0e ||
	c === 0xfe0f ||
	c === 0x20e3 ||
	(c >= 0x1f3fb && c <= 0x1f3ff) ||
	(c >= 0xe0020 && c <= 0xe007f);
const isRegional = (c: number) => c >= 0x1f1e6 && c <= 0x1f1ff;

/** Client-side mirror of the server's rule: one emoji grapheme, ZWJ sequences,
 *  skin tones and flags allowed. A blank value is valid — it clears the glyph
 *  back to the letter square. */
export function isValidAccountEmoji(value: string): boolean {
	const trimmed = (value ?? '').trim();
	if (!trimmed) return true;
	const points = [...trimmed].map((c) => c.codePointAt(0) ?? 0);
	if (points.length > EMOJI_MAX_SCALARS) return false;
	if (isRegional(points[0])) return points.length === 2 && isRegional(points[1]);
	let wantBase = true;
	for (const c of points) {
		if (wantBase) {
			if (!isBase(c)) return false;
			wantBase = false;
		} else if (c === 0x200d) {
			wantBase = true;
		} else if (!isModifier(c)) {
			return false;
		}
	}
	return !wantBase;
}
