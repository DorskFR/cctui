export const TILES_KEY = 'cctui_tiles';

export interface TilesDeps {
	/** `localStorage`-shaped, injected so tests need no browser. */
	store: { get: (k: string) => string; set: (k: string, v: string) => void };
	maxTiles: () => number;
	/** Over-cap adds nothing; the user is told rather than silently ignored. */
	onOverCap: (max: number) => void;
	/** Mirrors the open set into `/tiles?s=…` so the view is shareable and Back
	 *  works. Omitted in tests. */
	writeUrl?: (ids: string[]) => void;
}

export function parseTileIds(raw: string | null | undefined): string[] {
	if (!raw) return [];
	const out: string[] = [];
	for (const part of raw.split(',')) {
		const id = part.trim();
		if (id && !out.includes(id)) out.push(id);
	}
	return out;
}

/**
 * The tiles workspace: an ordered list of session ids, which one has focus, and
 * which one (if any) is maximised. A session appears at most once.
 *
 * Opening a tile is read-only attachment — subscribe plus a history fetch.
 * Nothing here may send a reply or resume a session: a view that opens nine
 * conversations at once must not cost nine turns.
 */
export class TilesWorkspace {
	#d: TilesDeps;

	ids = $state<string[]>([]);
	focused = $state<string | null>(null);
	maximized = $state<string | null>(null);

	/** What the grid renders when the viewport is wide enough: one tile while
	 *  maximised, every tile otherwise. */
	visible = $derived(this.maximized ? [this.maximized] : this.ids);

	/** A getter, not `$derived`: a field initializer cannot read the deps the
	 *  constructor has yet to assign. */
	get full(): boolean {
		return this.ids.length >= this.#d.maxTiles();
	}

	constructor(d: TilesDeps) {
		this.#d = d;
	}

	/** URL first (a shared link wins), then the persisted set. */
	hydrate = (fromUrl: string | null | undefined): void => {
		const fromLink = parseTileIds(fromUrl);
		const ids = fromLink.length ? fromLink : parseTileIds(this.#d.store.get(TILES_KEY));
		this.ids = ids.slice(0, this.#d.maxTiles());
		this.focused = this.ids[0] ?? null;
		this.#commit();
	};

	add = (id: string): boolean => {
		if (!id) return false;
		if (this.ids.includes(id)) {
			this.focus(id);
			return true;
		}
		if (this.ids.length >= this.#d.maxTiles()) {
			this.#d.onOverCap(this.#d.maxTiles());
			return false;
		}
		this.ids = [...this.ids, id];
		this.focused = id;
		this.#commit();
		return true;
	};

	/** Adds what fits and reports the cap once for the whole batch. */
	addMany = (ids: string[]): number => {
		let added = 0;
		for (const id of ids) {
			if (this.ids.includes(id)) continue;
			if (this.ids.length >= this.#d.maxTiles()) {
				this.#d.onOverCap(this.#d.maxTiles());
				break;
			}
			this.ids = [...this.ids, id];
			added++;
		}
		if (added > 0) {
			this.focused = this.ids.at(-1) ?? null;
			this.#commit();
		}
		return added;
	};

	remove = (id: string): void => {
		const at = this.ids.indexOf(id);
		if (at < 0) return;
		this.ids = this.ids.filter((x) => x !== id);
		if (this.maximized === id) this.maximized = null;
		if (this.focused === id) this.focused = this.ids[Math.min(at, this.ids.length - 1)] ?? null;
		this.#commit();
	};

	clear = (): void => {
		this.ids = [];
		this.focused = null;
		this.maximized = null;
		this.#commit();
	};

	/** Reorder by `delta` places, clamped at both ends. */
	move = (id: string, delta: number): void => {
		const from = this.ids.indexOf(id);
		if (from < 0) return;
		const to = Math.min(this.ids.length - 1, Math.max(0, from + delta));
		if (to === from) return;
		const next = [...this.ids];
		next.splice(to, 0, ...next.splice(from, 1));
		this.ids = next;
		this.#commit();
	};

	focus = (id: string): void => {
		if (this.ids.includes(id)) this.focused = id;
	};

	toggleMaximize = (id: string): void => {
		this.maximized = this.maximized === id ? null : this.ids.includes(id) ? id : null;
		if (this.maximized) this.focused = this.maximized;
	};

	restore = (): void => {
		this.maximized = null;
	};

	#commit(): void {
		this.#d.store.set(TILES_KEY, this.ids.join(','));
		this.#d.writeUrl?.(this.ids);
	}
}
