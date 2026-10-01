import { isArchiveChord, isFindChord } from '$lib/platform';

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
