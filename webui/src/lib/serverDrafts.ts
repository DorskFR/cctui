import { browser } from '$app/environment';
import { api } from './api';
import type { DraftList } from '@bindings/DraftList';
import type { PutDraftRequest } from '@bindings/PutDraftRequest';
import { type DraftRemote, drafts, hydrateLocal, localRoamingKeys, setDraftRemote } from './drafts';

/** Set once this browser's pre-existing drafts have been pushed up, so a later
 *  visit does not resurrect a draft the user deleted on another device. */
const IMPORTED = 'cctui_drafts_imported';

const SAVE_DEBOUNCE_MS = 700;

/** A draft key travels as one path segment; percent-encoding keeps the unit
 *  separators and working-directory slashes inside spawn-slot keys intact. */
function draftPath(key: string): string {
	return `/drafts/${encodeURIComponent(key)}`;
}

class ServerDrafts implements DraftRemote {
	#timers = new Map<string, ReturnType<typeof setTimeout>>();
	#pending = new Map<string, string>();
	#loading: Promise<void> | null = null;

	/** Push a roaming key, debounced per key so a keystroke is not a request. */
	put(key: string, text: string) {
		if (!browser) return;
		this.#pending.set(key, text);
		const existing = this.#timers.get(key);
		if (existing) clearTimeout(existing);
		this.#timers.set(
			key,
			setTimeout(() => this.#send(key), SAVE_DEBOUNCE_MS)
		);
	}

	del(key: string) {
		if (!browser) return;
		this.#cancel(key);
		void api.del(draftPath(key)).catch(() => {
			/* offline / already gone — the local mirror is already cleared */
		});
	}

	#cancel(key: string) {
		const t = this.#timers.get(key);
		if (t) clearTimeout(t);
		this.#timers.delete(key);
		this.#pending.delete(key);
	}

	#send(key: string, keepalive = false) {
		const text = this.#pending.get(key);
		this.#timers.delete(key);
		this.#pending.delete(key);
		if (text === undefined) return;
		const body: PutDraftRequest = { text };
		void api.put(draftPath(key), body, keepalive ? { keepalive: true } : undefined).catch(() => {
			/* offline / 401 — the local mirror still holds the text */
		});
	}

	/** Send every queued write immediately; without this a reload inside the
	 *  debounce window loses the draft the server never heard about. */
	flush() {
		for (const [key, timer] of [...this.#timers]) {
			clearTimeout(timer);
			this.#send(key, true);
		}
	}

	/** Adopt the server's drafts into the local mirror, then push up whatever
	 *  this browser had first. Safe to call repeatedly; runs once. */
	load(): Promise<void> {
		if (!browser) return Promise.resolve();
		this.#loading ??= this.#pull();
		return this.#loading;
	}

	get ready(): Promise<void> {
		return this.#loading ?? Promise.resolve();
	}

	async #pull(): Promise<void> {
		// Snapshot before hydrating: afterwards a server key is indistinguishable
		// from one this browser already had.
		const local = localRoamingKeys();
		let list: DraftList;
		try {
			list = await api.get<DraftList>('/drafts');
		} catch {
			return;
		}
		const onServer = new Set<string>();
		for (const d of list.drafts) {
			onServer.add(d.key);
			// Adopting the server's text makes a write queued before the load stale:
			// letting it fire would push this tab's older copy back over it.
			this.#cancel(d.key);
			hydrateLocal(d.key, d.text);
		}
		if (localStorage.getItem(IMPORTED)) return;
		for (const key of local) {
			if (onServer.has(key)) continue;
			const text = drafts.get(key);
			if (!text) continue;
			this.#pending.set(key, text);
			this.#send(key);
		}
		hydrateLocal(IMPORTED, '1');
	}
}

export const serverDrafts = new ServerDrafts();

if (browser) {
	setDraftRemote(serverDrafts);
	const flush = () => serverDrafts.flush();
	window.addEventListener('pagehide', flush);
	document.addEventListener('visibilitychange', () => {
		if (document.visibilityState === 'hidden') flush();
	});
}
