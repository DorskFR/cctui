/** Must match the daemon's placeholder byte for byte. */
export const USER_REDACTED = '[REDACTED:user]';

const SPAN = /<redacted>([\s\S]*?)<\/redacted>/gi;
const OPEN = /<redacted>/i;
const CLOSE = /<\/redacted>/i;

export interface RedactedSpan {
	start: number;
	end: number;
	value: string;
}

export function redactedSpans(text: string): RedactedSpan[] {
	const out: RedactedSpan[] = [];
	for (const m of text.matchAll(SPAN)) {
		if (!m[1]) continue;
		out.push({ start: m.index, end: m.index + m[0].length, value: m[1] });
	}
	return out;
}

export function redactUserSpans(text: string): string {
	if (!text) return text;
	return text.replace(SPAN, (whole, inner: string) => (inner ? USER_REDACTED : whole));
}

export function hasUnclosedTag(text: string): boolean {
	return OPEN.test(text.replace(SPAN, ''));
}

export function wrapRedacted(
	text: string,
	start: number,
	end: number
): { text: string; selStart: number; selEnd: number } {
	const a = Math.max(0, Math.min(start, end, text.length));
	const b = Math.min(text.length, Math.max(start, end));
	const open = '<redacted>';
	const next = `${text.slice(0, a)}${open}${text.slice(a, b)}</redacted>${text.slice(b)}`;
	return { text: next, selStart: a + open.length, selEnd: b + open.length };
}

/** JSON documents are masked string by string, so a span cannot straddle two
 *  fields and corrupt the document. */
export function redactStoredText(text: string): string {
	if (!text || !CLOSE.test(text)) return text;
	let parsed: unknown;
	try {
		parsed = JSON.parse(text);
	} catch {
		return redactUserSpans(text);
	}
	if (parsed === null || typeof parsed !== 'object') return redactUserSpans(text);
	return JSON.stringify(redactDeep(parsed));
}

function redactDeep(v: unknown): unknown {
	if (typeof v === 'string') return redactUserSpans(v);
	if (Array.isArray(v)) return v.map(redactDeep);
	if (v && typeof v === 'object') {
		return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, redactDeep(x)]));
	}
	return v;
}
