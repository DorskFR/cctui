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

/** True when a keydown event carries the platform submit chord (Ctrl/⌘ + Enter).
 *  Both modifiers are accepted on every platform so external keyboards work. */
export function isSubmitChord(e: KeyboardEvent): boolean {
	return e.key === 'Enter' && (e.metaKey || e.ctrlKey);
}
