import type { Bookmark } from '@bindings/Bookmark';
import type { CreateBookmark } from '@bindings/CreateBookmark';
import { lineMarkdown } from '$lib/components/organisms/conversation/format';
import type { Line } from '$lib/components/organisms/conversation/types';

const TITLE_MAX = 120;

/** `seq` is stamped onto lines by the conversation builder (CCT-990); it is
 * absent on older/unstamped lines, which still bookmark fine as a
 * session-only back-link. */
type SeqLine = Line & { seq?: number | null };

export function lineSeq(ln: Line): number | null {
	return (ln as SeqLine).seq ?? null;
}

/** The payload a save posts: the message text is snapshotted, not referenced. */
export function draftFromLine(
	ln: Line,
	sessionId: string,
	sessionName: string | null
): CreateBookmark {
	const body = lineMarkdown(ln);
	return {
		session_id: sessionId,
		seq: lineSeq(ln),
		message_id: ln.messageId ?? null,
		title: defaultTitle(body),
		body,
		role: ln.role,
		session_name: sessionName,
		note: null,
		message_ts: ln.ts
	};
}

/** The session's newest assistant message — the wrap-up the one-click save targets. */
export function lastAssistantLine(lines: Line[]): Line | null {
	for (let i = lines.length - 1; i >= 0; i--) {
		if (lines[i].role === 'assistant' && (lines[i].text ?? '').trim() !== '') return lines[i];
	}
	return null;
}

/**
 * Whether a line already has at least one bookmark pointing at it. Saving is
 * not a toggle, so this only drives the "saved" glyph.
 */
export function isLineBookmarked(
	bookmarks: Bookmark[],
	sessionId: string,
	ln: Line
): Bookmark | null {
	const seq = lineSeq(ln);
	return (
		bookmarks.find(
			(b) =>
				b.session_id === sessionId &&
				((seq !== null && b.seq === seq) ||
					(ln.messageId != null && b.message_id === ln.messageId))
		) ?? null
	);
}

/** Default bookmark title: the message's first non-empty line, trimmed. */
export function defaultTitle(body: string): string {
	const line = (body ?? '')
		.split('\n')
		.map((l) => l.trim())
		.find((l) => l.length > 0);
	if (!line) return '';
	if ([...line].length <= TITLE_MAX) return line;
	return `${[...line].slice(0, TITLE_MAX - 1).join('').trimEnd()}…`;
}

/** A bookmark whose source session has been deleted (`ON DELETE SET NULL`). */
export function isDeadLink(b: Bookmark): boolean {
	return b.session_id === null;
}

/**
 * Target URL for "Open session". The `seq` param is the seam the
 * `focusSeq` / `ensureSeqVisible(seq)` primitive (CCT-990/CCT-991) reads to
 * scroll the drawer to the source message; without it the drawer simply opens.
 */
export function sourceHref(b: Bookmark): string | null {
	if (b.session_id === null) return null;
	const base = `/sessions/${encodeURIComponent(b.session_id)}`;
	return b.seq === null ? base : `${base}?seq=${b.seq}`;
}

/** Markdown for the clipboard: the title as a heading, then note, then body. */
export function bookmarkMarkdown(b: Bookmark): string {
	const parts = [`# ${b.title}`];
	if (b.note) parts.push(`> ${b.note}`);
	parts.push(b.body);
	return parts.join('\n\n');
}

/** Free-text terms of a query, for `highlightTerms`; quoted phrases stay whole. */
export function queryTerms(q: string): string[] {
	return [...(q ?? '').matchAll(/"([^"]+)"|(\S+)/g)]
		.map((mm) => (mm[1] ?? mm[2] ?? '').trim())
		.filter(Boolean);
}
