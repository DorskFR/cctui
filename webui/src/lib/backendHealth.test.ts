import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
	BackendHealth,
	DEBOUNCE_MS,
	QUIET_MS,
	RETRY_MAX_MS,
	isNetworkError,
	isOutageStatus,
	retryDelay
} from './backendHealth.svelte';

beforeEach(() => {
	vi.useFakeTimers();
	vi.setSystemTime(0);
});
afterEach(() => vi.useRealTimers());

function healthWith(answers: boolean[]) {
	const probe = vi.fn(async () => answers.shift() ?? true);
	return { health: new BackendHealth(probe), probe };
}

describe('classification', () => {
	it('only gateway statuses count as an outage', () => {
		expect([502, 503, 504].every(isOutageStatus)).toBe(true);
		expect([400, 401, 403, 404, 409, 500].some(isOutageStatus)).toBe(false);
	});

	it('a fetch rejection is a network error, an abort is not', () => {
		expect(isNetworkError(new TypeError('Failed to fetch'))).toBe(true);
		expect(isNetworkError(new DOMException('aborted', 'AbortError'))).toBe(false);
	});

	it('backs off exponentially up to the cap', () => {
		expect([0, 1, 2, 3].map(retryDelay)).toEqual([1_000, 2_000, 4_000, 8_000]);
		expect(retryDelay(20)).toBe(RETRY_MAX_MS);
	});
});

describe('BackendHealth', () => {
	it('a single transient failure does not flip to down when the probe answers', async () => {
		const { health, probe } = healthWith([true]);
		health.reportFailure();
		expect(health.state).toBe('suspect');
		expect(probe).not.toHaveBeenCalled();
		await vi.advanceTimersByTimeAsync(DEBOUNCE_MS);
		expect(probe).toHaveBeenCalledTimes(1);
		expect(health.state).toBe('up');
	});

	it('a request succeeding during the debounce clears the suspicion without probing', async () => {
		const { health, probe } = healthWith([]);
		health.reportFailure();
		health.reportOk();
		await vi.advanceTimersByTimeAsync(DEBOUNCE_MS * 2);
		expect(probe).not.toHaveBeenCalled();
		expect(health.state).toBe('up');
	});

	it('goes down only after the debounced probe fails, then retries with backoff', async () => {
		const { health, probe } = healthWith([false, false, false]);
		health.reportFailure();
		await vi.advanceTimersByTimeAsync(DEBOUNCE_MS - 1);
		expect(health.state).toBe('suspect');
		await vi.advanceTimersByTimeAsync(1);
		expect(health.state).toBe('down');
		expect(health.nextRetryAt).toBe(Date.now() + 1_000);

		await vi.advanceTimersByTimeAsync(1_000);
		expect(probe).toHaveBeenCalledTimes(2);
		expect(health.nextRetryAt).toBe(Date.now() + 2_000);

		await vi.advanceTimersByTimeAsync(2_000);
		expect(probe).toHaveBeenCalledTimes(3);
		expect(health.nextRetryAt).toBe(Date.now() + 4_000);
	});

	it('recovers once the server answers and notifies listeners and waiters', async () => {
		const { health } = healthWith([false, false, true]);
		const recovered = vi.fn();
		health.onRecover(recovered);
		health.reportFailure();
		const waited = vi.fn();
		void health.waitUntilUp().then(waited);
		await vi.advanceTimersByTimeAsync(DEBOUNCE_MS + 1_000);
		expect(health.state).toBe('down');
		expect(recovered).not.toHaveBeenCalled();

		await vi.advanceTimersByTimeAsync(2_000);
		expect(health.state).toBe('up');
		expect(health.nextRetryAt).toBeNull();
		expect(recovered).toHaveBeenCalledTimes(1);
		expect(waited).toHaveBeenCalledTimes(1);
	});

	it('retryNow probes immediately while down', async () => {
		const { health, probe } = healthWith([false, true]);
		health.reportFailure();
		await vi.advanceTimersByTimeAsync(DEBOUNCE_MS);
		expect(health.state).toBe('down');
		health.retryNow();
		await vi.advanceTimersByTimeAsync(0);
		expect(probe).toHaveBeenCalledTimes(2);
		expect(health.state).toBe('up');
	});

	it('ignores failure signals for a quiet window after a good probe', async () => {
		const { health, probe } = healthWith([true, true]);
		health.reportFailure();
		await vi.advanceTimersByTimeAsync(DEBOUNCE_MS);
		health.reportFailure();
		expect(health.state).toBe('up');
		await vi.advanceTimersByTimeAsync(QUIET_MS);
		health.reportFailure();
		expect(health.state).toBe('suspect');
		await vi.advanceTimersByTimeAsync(DEBOUNCE_MS);
		expect(probe).toHaveBeenCalledTimes(2);
	});

	it('waitUntilUp probes even inside the quiet window', async () => {
		const { health, probe } = healthWith([true, true]);
		health.reportFailure();
		await vi.advanceTimersByTimeAsync(DEBOUNCE_MS);
		const waited = vi.fn();
		void health.waitUntilUp().then(waited);
		await vi.advanceTimersByTimeAsync(DEBOUNCE_MS);
		expect(probe).toHaveBeenCalledTimes(2);
		expect(waited).toHaveBeenCalledTimes(1);
	});
});
