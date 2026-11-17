import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError, FeatureOffError } from '#lib/api.ts';
import { cwdCheckOf, makeCwdCheckWatcher, type CwdCheck } from './cwdCheck';

describe('cwdCheckOf', () => {
	it('reads a 404 as missing and a 503 as offline, and nothing else as either', () => {
		expect(cwdCheckOf(new ApiError(404, 'cannot resolve /nope'))).toBe('missing');
		expect(cwdCheckOf(new ApiError(503, 'daemon offline'))).toBe('offline');
		expect(cwdCheckOf(new ApiError(400, '/etc is outside the allowed roots'))).toBe('unknown');
		expect(cwdCheckOf(new ApiError(504, 'timed out'))).toBe('unknown');
		expect(cwdCheckOf(new FeatureOffError('files'))).toBe('unknown');
		expect(cwdCheckOf(new TypeError('boom'))).toBe('unknown');
	});
});

describe('makeCwdCheckWatcher', () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	function watch(lookup: (m: string, p: string) => Promise<unknown>) {
		const results: CwdCheck[] = [];
		const w = makeCwdCheckWatcher(lookup, (c) => results.push(c), 300);
		return { w, results };
	}

	it('debounces and reports only the latest path', async () => {
		const lookup = vi.fn(async (_m: string, p: string) => {
			if (p === '/nope') throw new ApiError(404, 'missing');
			return { dirs: [] };
		});
		const { w, results } = watch(lookup);
		w.update('m1', '/w', 'online');
		w.update('m1', ' /nope ', 'online');
		await vi.advanceTimersByTimeAsync(299);
		expect(lookup).not.toHaveBeenCalled();
		await vi.advanceTimersByTimeAsync(1);
		expect(lookup).toHaveBeenCalledTimes(1);
		expect(lookup).toHaveBeenCalledWith('m1', '/nope');
		expect(results).toEqual(['unknown', 'unknown', 'missing']);

		w.update('m1', '/w', 'online');
		await vi.advanceTimersByTimeAsync(300);
		expect(results.at(-1)).toBe('exists');
	});

	it('never asks a machine known offline and reports it as such', async () => {
		const lookup = vi.fn(async () => ({ dirs: [] }));
		const { w, results } = watch(lookup);
		w.update('m1', '/w', 'offline');
		await vi.advanceTimersByTimeAsync(300);
		expect(lookup).not.toHaveBeenCalled();
		expect(results).toEqual(['offline']);
	});

	it('stays unknown without a machine, a path or a liveness, and on a lookup that throws', async () => {
		const lookup = vi.fn(() => {
			throw new Error('not wired');
		});
		const { w, results } = watch(lookup);
		w.update('', '/w', 'online');
		w.update('m1', '  ', 'online');
		w.update('m1', '/w', undefined);
		expect(results).toEqual(['unknown', 'unknown', 'unknown']);
		w.update('m1', '/w', 'online');
		await vi.advanceTimersByTimeAsync(300);
		expect(lookup).toHaveBeenCalledTimes(1);
		expect(results).toEqual(['unknown', 'unknown', 'unknown', 'unknown', 'unknown']);
	});

	it('drops a reply that lands after cancel', async () => {
		let release: () => void = () => {};
		const lookup = vi.fn(
			() =>
				new Promise((_, reject) => {
					release = () => reject(new ApiError(404, 'missing'));
				})
		);
		const { w, results } = watch(lookup);
		w.update('m1', '/nope', 'online');
		await vi.advanceTimersByTimeAsync(300);
		w.cancel();
		release();
		await vi.advanceTimersByTimeAsync(0);
		expect(results).toEqual(['unknown']);
	});
});
