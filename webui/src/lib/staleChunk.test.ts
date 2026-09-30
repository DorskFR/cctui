import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
	isStaleChunkError,
	recoverFromStaleChunk,
	resetStaleChunkMemoryGuard,
	STALE_CHUNK_COOLDOWN_MS,
	STALE_CHUNK_KEY
} from './staleChunk';

function fakeStorage(seed: Record<string, string> = {}) {
	const map = new Map(Object.entries(seed));
	return {
		map,
		getItem: (k: string) => map.get(k) ?? null,
		setItem: (k: string, v: string) => void map.set(k, v)
	};
}

describe('isStaleChunkError', () => {
	it('matches the chunk-load failure of every browser', () => {
		expect(
			isStaleChunkError(
				'Failed to fetch dynamically imported module: https://cctui.dorsk.dev/_app/immutable/nodes/2.DEufLG1w.js'
			)
		).toBe(true);
		expect(isStaleChunkError('Importing a module script failed.')).toBe(true);
		expect(isStaleChunkError('error loading dynamically imported module: /_app/x.js')).toBe(true);
		expect(isStaleChunkError('FAILED TO FETCH DYNAMICALLY IMPORTED MODULE')).toBe(true);
	});

	it('leaves real failures alone', () => {
		expect(isStaleChunkError('TypeError: x is not a function')).toBe(false);
		expect(isStaleChunkError('Failed to fetch')).toBe(false);
		expect(isStaleChunkError('NetworkError when attempting to fetch resource.')).toBe(false);
		expect(isStaleChunkError('')).toBe(false);
	});
});

describe('recoverFromStaleChunk', () => {
	beforeEach(() => resetStaleChunkMemoryGuard());

	it('reloads once and stamps the guard', () => {
		const storage = fakeStorage();
		const reload = vi.fn();
		expect(recoverFromStaleChunk({ storage, reload, now: () => 1_000 })).toBe(true);
		expect(reload).toHaveBeenCalledTimes(1);
		expect(storage.map.get(STALE_CHUNK_KEY)).toBe('1000');
	});

	it('refuses a second reload inside the cooldown so an outage cannot loop', () => {
		const storage = fakeStorage();
		const reload = vi.fn();
		recoverFromStaleChunk({ storage, reload, now: () => 1_000 });
		expect(
			recoverFromStaleChunk({ storage, reload, now: () => 1_000 + STALE_CHUNK_COOLDOWN_MS - 1 })
		).toBe(false);
		expect(reload).toHaveBeenCalledTimes(1);
	});

	it('allows a later deploy to reload again once the cooldown has passed', () => {
		const storage = fakeStorage();
		const reload = vi.fn();
		recoverFromStaleChunk({ storage, reload, now: () => 1_000 });
		expect(
			recoverFromStaleChunk({ storage, reload, now: () => 1_000 + STALE_CHUNK_COOLDOWN_MS })
		).toBe(true);
		expect(reload).toHaveBeenCalledTimes(2);
	});

	it('ignores an unparseable guard value', () => {
		const storage = fakeStorage({ [STALE_CHUNK_KEY]: 'nonsense' });
		const reload = vi.fn();
		expect(recoverFromStaleChunk({ storage, reload, now: () => 5_000 })).toBe(true);
		expect(reload).toHaveBeenCalledTimes(1);
	});

	it('still guards when storage is unavailable', () => {
		const reload = vi.fn();
		expect(recoverFromStaleChunk({ storage: null, reload, now: () => 1_000 })).toBe(true);
		expect(recoverFromStaleChunk({ storage: null, reload, now: () => 2_000 })).toBe(false);
		expect(reload).toHaveBeenCalledTimes(1);
	});
});
