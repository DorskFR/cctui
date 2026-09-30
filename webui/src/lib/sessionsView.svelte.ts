import { browser } from '$app/environment';
import { drafts, LIST_VIEW } from './drafts';

export type ViewMode = 'list' | 'grid' | 'tiles';

export const VIEW_MODES: ViewMode[] = ['list', 'grid', 'tiles'];
/** Tiles need a window to split; below this the option is not offered. */
export const TILES_MIN_WIDTH = 960;

export const isViewMode = (v: string): v is ViewMode =>
	v === 'list' || v === 'grid' || v === 'tiles';

/** `card` is the legacy name for `grid`, still written so an existing
 *  preference survives a downgrade. */
export function parseViewMode(raw: string | null | undefined): ViewMode {
	if (raw === 'card') return 'grid';
	return raw && isViewMode(raw) ? raw : 'list';
}
export const serializeViewMode = (v: ViewMode): string => (v === 'grid' ? 'card' : v);

/**
 * The Sessions view mode, owned by a module rather than by the Sessions page.
 *
 * The app layout has to know whether tiles are on, because tiles opt out of the
 * width-capped content column. Publishing that from inside the page does not
 * work: the layout renders the page *inside* the branch it would toggle, so the
 * page unmounts itself the moment it raises the flag, its teardown lowers it,
 * and the two flip forever. Keeping the mode here gives both the layout and the
 * page one owner whose lifetime is the app's.
 */
class SessionsView {
	#mode = $state<ViewMode>('list');
	wide = $state(true);

	constructor() {
		if (!browser) return;
		this.#mode = parseViewMode(drafts.get(LIST_VIEW));
		const mq = window.matchMedia(`(min-width: ${TILES_MIN_WIDTH}px)`);
		this.wide = mq.matches;
		mq.addEventListener('change', (e) => (this.wide = e.matches));
	}

	// Sole owner of the value AND of persisting it: a second owner that re-seeds
	// on mount fights whoever just changed the mode.
	get mode(): ViewMode {
		return this.#mode;
	}
	set mode(v: ViewMode) {
		this.#mode = v;
		drafts.set(LIST_VIEW, serializeViewMode(v));
	}

	/** A stored tiles choice is kept but not honoured on a narrow viewport. */
	get effective(): ViewMode {
		return this.mode === 'tiles' && !this.wide ? 'list' : this.mode;
	}
	get tiles(): boolean {
		return this.effective === 'tiles';
	}
}

export const sessionsView = new SessionsView();
