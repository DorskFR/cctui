// A tab that outlives a deploy still knows only the old content-hashed chunk
// names, so its next lazy `import()` 404s against a build that no longer
// exists. Reload into the new build rather than surfacing the fetch error.

// Chrome, Safari and Firefox each word a failed dynamic import differently.
const CHUNK_MESSAGES = [
	/failed to fetch dynamically imported module/i,
	/importing a module script failed/i,
	/error loading dynamically imported module/i
];

export function isStaleChunkError(text: string): boolean {
	return CHUNK_MESSAGES.some((re) => re.test(text));
}

export const STALE_CHUNK_KEY = 'cctui:stale-chunk-reload';

/** Two failures inside this window mean the reload did not help (a real
 *  outage, not a deploy), so the second one is surfaced instead of looping. */
export const STALE_CHUNK_COOLDOWN_MS = 60_000;

export type StaleChunkDeps = {
	storage?: Pick<Storage, 'getItem' | 'setItem'> | null;
	reload?: () => void;
	now?: () => number;
};

let memoryGuard: string | null = null;

const memoryStorage: Pick<Storage, 'getItem' | 'setItem'> = {
	getItem: (k) => (k === STALE_CHUNK_KEY ? memoryGuard : null),
	setItem: (k, v) => {
		if (k === STALE_CHUNK_KEY) memoryGuard = v;
	}
};

function defaultStorage(): Pick<Storage, 'getItem' | 'setItem'> {
	try {
		if (typeof sessionStorage !== 'undefined') {
			sessionStorage.getItem(STALE_CHUNK_KEY);
			return sessionStorage;
		}
	} catch {
		/* blocked storage */
	}
	return memoryStorage;
}

/**
 * Reload once into the new build. Returns true when the reload was triggered
 * (the caller should stay silent), false when the guard is still warm and the
 * error deserves a toast after all.
 */
export function recoverFromStaleChunk(deps: StaleChunkDeps = {}): boolean {
	const now = deps.now ?? (() => Date.now());
	const store = deps.storage === undefined ? defaultStorage() : (deps.storage ?? memoryStorage);
	const at = now();

	let previous: number | null = null;
	try {
		const raw = store.getItem(STALE_CHUNK_KEY);
		if (raw) previous = Number(raw);
	} catch {
		previous = null;
	}
	if (previous !== null && Number.isFinite(previous) && at - previous < STALE_CHUNK_COOLDOWN_MS) {
		return false;
	}

	try {
		store.setItem(STALE_CHUNK_KEY, String(at));
	} catch {
		memoryStorage.setItem(STALE_CHUNK_KEY, String(at));
	}

	const reload = deps.reload ?? (() => location.reload());
	reload();
	return true;
}

export function resetStaleChunkMemoryGuard(): void {
	memoryGuard = null;
}
