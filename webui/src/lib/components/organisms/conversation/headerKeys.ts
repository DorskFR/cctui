import { isArchiveChord, isFindChord } from '#lib/platform.ts';

export type HeaderKeyAction = 'search' | 'archive' | 'escape';

export type HeaderKeyState = {
	/** False for every tile but the active one, so N tiles fire one handler. */
	active: boolean;
	renaming: boolean;
	archived: boolean;
	canSearch: boolean;
	archiveShortcut: boolean;
};

export function headerKeyAction(e: KeyboardEvent, s: HeaderKeyState): HeaderKeyAction | null {
	if (!s.active || s.renaming) return null;
	if (s.canSearch && isFindChord(e)) return 'search';
	if (!s.archived && s.archiveShortcut && isArchiveChord(e)) return 'archive';
	if (e.key === 'Escape') return 'escape';
	return null;
}

export type HeaderKeyHandlers = {
	onsearch?: () => void;
	onstoparchive: () => void;
	/** First refusal on Escape: true when it consumed the key. */
	onescape?: () => boolean;
	/** What an unconsumed Escape does: close the drawer, or interrupt a tile. */
	onescapeaction?: () => void;
};

export function runHeaderKey(e: KeyboardEvent, s: HeaderKeyState, h: HeaderKeyHandlers): void {
	const action = headerKeyAction(e, s);
	if (action === 'search') {
		e.preventDefault();
		h.onsearch?.();
	} else if (action === 'archive') {
		e.preventDefault();
		h.onstoparchive();
	} else if (action === 'escape') {
		if (h.onescape?.()) e.preventDefault();
		else h.onescapeaction?.();
	}
}
