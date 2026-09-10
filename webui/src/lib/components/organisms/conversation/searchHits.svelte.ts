// Prev/next stepping over the `<mark class="search-hit">` occurrences that the
// `highlight` terms produced, counted from the DOM of the currently rendered
// lines — the marks are injected into pre-rendered HTML, so there is no line
// model to count them from.

export const HIT_SELECTOR = 'mark.search-hit';

export class SearchHitStepper {
	#scroller: () => HTMLElement | undefined;
	#loadOlder: () => Promise<void> | void;

	count = $state(0);
	index = $state(-1);

	constructor(opts: {
		scroller: () => HTMLElement | undefined;
		loadOlder: () => Promise<void> | void;
	}) {
		this.#scroller = opts.scroller;
		this.#loadOlder = opts.loadOlder;
	}

	#hits(): HTMLElement[] {
		const el = this.#scroller();
		if (!el) return [];
		return [...el.querySelectorAll<HTMLElement>(HIT_SELECTOR)];
	}

	// Re-anchors on the current hit by identity: prepending older lines shifts
	// every index, and renumbering under the user loses their position.
	refresh = () => {
		const hits = this.#hits();
		const current = this.#current;
		this.count = hits.length;
		if (current && hits.includes(current)) this.index = hits.indexOf(current);
		else if (this.index >= hits.length) this.index = hits.length - 1;
	};

	#current: HTMLElement | null = null;

	#select(hits: HTMLElement[], i: number) {
		const el = hits[i];
		if (!el) return;
		this.index = i;
		this.#current = el;
		for (const h of hits) h.classList.toggle('hit-current', h === el);
		el.scrollIntoView({ block: 'center', behavior: 'smooth' });
	}

	next = () => {
		const hits = this.#hits();
		this.count = hits.length;
		if (hits.length === 0) return;
		this.#select(hits, Math.min(this.index + 1, hits.length - 1));
	};

	prev = async () => {
		let hits = this.#hits();
		this.count = hits.length;
		if (hits.length === 0) return;
		if (this.index <= 0) {
			const before = hits.length;
			await this.#loadOlder();
			hits = this.#hits();
			this.count = hits.length;
			// Older lines prepend, so the previously-first hit shifted right by the
			// number of hits that appeared above it.
			const grew = hits.length - before;
			if (grew > 0) {
				this.#select(hits, Math.max(grew - 1, 0));
				return;
			}
			this.#select(hits, 0);
			return;
		}
		this.#select(hits, this.index - 1);
	};

	reset = () => {
		this.count = 0;
		this.index = -1;
		this.#current = null;
	};
}
