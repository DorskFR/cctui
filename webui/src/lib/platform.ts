// Platform helpers for keyboard-shortcut affordances.

/** True on macOS/iPad, where the platform submit chord is ⌘+Enter rather than
 *  Ctrl+Enter (the Windows/Linux standard). Best-effort UA sniff; falls back to
 *  the non-Mac label when `navigator` is unavailable (SSR). */
export function isMac(): boolean {
	if (typeof navigator === 'undefined') return false;
	const p = navigator.platform || '';
	const ua = navigator.userAgent || '';
	return /Mac|iPhone|iPad|iPod/i.test(p) || /Mac/i.test(ua);
}

/** Human label for the "submit this form" chord, platform-aware:
 *  `⌘ Enter` on Mac, `Ctrl + Enter` elsewhere. */
export function submitChordLabel(): string {
	return isMac() ? '⌘ Enter' : 'Ctrl + Enter';
}

/** The same chord as separate keys, for a <Kbd> hint. */
export function submitChordKeys(): string[] {
	return isMac() ? ['⌘', '↩'] : ['Ctrl', '↩'];
}

/** True when a keydown event carries the platform submit chord (Ctrl/⌘ + Enter).
 *  Both modifiers are accepted on every platform so external keyboards work. */
export function isSubmitChord(e: KeyboardEvent): boolean {
	return e.key === 'Enter' && (e.metaKey || e.ctrlKey);
}

/** True when a keydown carries the platform "archive" chord — ⌘+E on Mac,
 *  Ctrl+E elsewhere (Beeper/Slack-style). Deliberately platform-EXCLUSIVE (not
 *  "either modifier" like the submit chord) so it doesn't clobber the other
 *  platform's native Ctrl+E text-editing binding (move-to-end-of-line on macOS).
 *  Alt/Shift combos are excluded so it never fires on a wider chord. */
export function isArchiveChord(e: KeyboardEvent): boolean {
	if (e.altKey || e.shiftKey) return false;
	if (e.key !== 'e' && e.key !== 'E') return false;
	return isMac() ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
}
