import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync } from 'svelte';
import { clockRunning, now } from './clock.svelte';

describe('shared clock', () => {
	beforeEach(() => {
		vi.useFakeTimers();
		vi.setSystemTime(0);
	});
	afterEach(() => vi.useRealTimers());

	it('runs no interval without subscribers', () => {
		expect(now(1_000)).toBe(Date.now());
		expect(clockRunning()).toBe(false);
	});

	it('shares one interval and notifies each cadence on its own boundary', () => {
		const seen = { fast: 0, slow: 0 };
		const spy = vi.spyOn(globalThis, 'setInterval');
		const stop = $effect.root(() => {
			$effect(() => {
				now(1_000);
				seen.fast++;
			});
			$effect(() => {
				now(30_000);
				seen.slow++;
			});
		});
		flushSync();
		expect(clockRunning()).toBe(true);
		expect(spy).toHaveBeenCalledTimes(1);
		vi.advanceTimersByTime(5_000);
		flushSync();
		expect(seen.fast).toBe(6);
		expect(seen.slow).toBe(1);
		vi.advanceTimersByTime(25_000);
		flushSync();
		expect(seen.slow).toBe(2);
		stop();
		expect(clockRunning()).toBe(false);
		spy.mockRestore();
	});
});
