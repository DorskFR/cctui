export interface HistoryNavHost {
	/** Entries most-recent-last, as stored by `drafts.ts`. */
	list: () => string[];
	value: () => string;
	setValue: (value: string) => void;
	el: () => HTMLTextAreaElement | null | undefined;
}

/** ArrowUp/ArrowDown recall over a prompt history, shared by the conversation
 * composer and the spawn form so both textareas behave identically.
 *
 * Index -1 means editing the live draft; 0..n-1 browses newest-first. Recall
 * only starts with the caret at the very start (and ends at the very end) so
 * it never fights normal multiline cursor movement. */
export class HistoryNav {
	#host: HistoryNavHost;
	#index = -1;
	#stash = '';

	constructor(host: HistoryNavHost) {
		this.#host = host;
	}

	get browsing(): boolean {
		return this.#index !== -1;
	}

	reset() {
		this.#index = -1;
	}

	resetAll() {
		this.#index = -1;
		this.#stash = '';
	}

	back() {
		const list = this.#host.list();
		if (list.length === 0) return;
		if (this.#index === -1) this.#stash = this.#host.value();
		this.#index = Math.min(this.#index + 1, list.length - 1);
		this.#host.setValue(list[list.length - 1 - this.#index]);
	}

	forward() {
		const list = this.#host.list();
		if (this.#index === -1) return;
		const next = this.#index - 1;
		if (next < 0) {
			this.#index = -1;
			this.#host.setValue(this.#stash);
		} else {
			this.#index = next;
			this.#host.setValue(list[list.length - 1 - next]);
		}
	}

	/** Jump straight to an entry (menu pick), stashing the live draft first. */
	recall(value: string) {
		if (this.#index === -1) this.#stash = this.#host.value();
		const list = this.#host.list();
		const at = list.lastIndexOf(value);
		this.#index = at === -1 ? this.#index : list.length - 1 - at;
		this.#host.setValue(value);
	}

	#caretAtStart(): boolean {
		const el = this.#host.el();
		return !!el && el.selectionStart === 0 && el.selectionEnd === 0;
	}

	#caretAtEnd(): boolean {
		const el = this.#host.el();
		const len = this.#host.value().length;
		return !!el && el.selectionStart === len && el.selectionEnd === len;
	}

	/** Returns true when the key was consumed (and already preventDefault'd). */
	handleKey(e: KeyboardEvent): boolean {
		if (e.key === 'ArrowUp' && (this.browsing || this.#caretAtStart())) {
			e.preventDefault();
			this.back();
			return true;
		}
		if (e.key === 'ArrowDown' && this.browsing && this.#caretAtEnd()) {
			e.preventDefault();
			this.forward();
			return true;
		}
		return false;
	}
}
