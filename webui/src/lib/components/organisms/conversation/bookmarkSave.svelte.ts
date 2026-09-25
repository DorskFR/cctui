import type { CreateBookmark } from '@bindings/CreateBookmark';
import { errMessage } from '$lib/api';
import { draftFromLine, isLineBookmarked } from '$lib/bookmarks';
import { useBookmarkActions, useBookmarks } from '$lib/queries';
import { toasts } from '$lib/toast.svelte';
import { m } from '$lib/paraglide/messages';
import type { Line } from './types';

export interface BookmarkSaverOpts {
	id: () => string;
	sessionName: () => string | null;
}

export class BookmarkSaver {
	#o: BookmarkSaverOpts;
	#saved = useBookmarks();
	#actions = useBookmarkActions();
	draft = $state<CreateBookmark | null>(null);

	constructor(o: BookmarkSaverOpts) {
		this.#o = o;
	}

	isBookmarked = (ln: Line): boolean =>
		isLineBookmarked(this.#saved.data ?? [], this.#o.id(), ln) !== null;

	open = (ln: Line): void => {
		this.draft = draftFromLine(ln, this.#o.id(), this.#o.sessionName());
	};

	close = (): void => {
		this.draft = null;
	};

	save = async (title: string, note: string | null): Promise<void> => {
		const draft = this.draft;
		this.draft = null;
		if (!draft) return;
		try {
			await this.#actions.create({ ...draft, title, note });
			toasts.ok(m.bookmarks_saved());
		} catch (e) {
			toasts.error(m.bookmarks_save_failed({ message: errMessage(e) }));
		}
	};
}
