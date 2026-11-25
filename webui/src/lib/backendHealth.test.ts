import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
	BackendHealth,
	DEBOUNCE_MS,
	QUIET_MS,
	RECONNECT_SCREEN_AFTER_MS,
	RETRY_MAX_MS,
	isNetworkError,
	isOutageStatus,
	outageView,
	retryCountdown,
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

describe('outage view', () => {
	it('maps state, contact and escalation to none, banner or screen', () => {
		expect(outageView('up', false, false)).toBe('none');
		expect(outageView('suspect', true, false)).toBe('none');
		expect(outageView('down', true, false)).toBe('reconnecting');
		expect(outageView('down', true, true)).toBe('offline');
		expect(outageView('down', false, false)).toBe('offline');
	});

	it('counts down whole seconds and reads as retrying when due or probing', () => {
		expect(retryCountdown(18_000, false, 10_000)).toBe(8);
		expect(retryCountdown(10_400, false, 10_000)).toBe(1);
		expect(retryCountdown(10_000, false, 10_000)).toBeNull();
		expect(retryCountdown(18_000, true, 10_000)).toBeNull();
		expect(retryCountdown(null, false, 10_000)).toBeNull();
	});

	const neverUp = () => new BackendHealth(vi.fn(async () => false));

	async function goDown(health: BackendHealth) {
		health.reportFailure();
		await vi.advanceTimersByTimeAsync(DEBOUNCE_MS);
		expect(health.state).toBe('down');
	}

	it('a fresh load that never reached the server goes straight to the screen', async () => {
		const health = neverUp();
		expect(health.connected).toBe(false);
		await goDown(health);
		expect(health.outage).toBe('offline');
	});

	it('losing a connected server shows the banner, then escalates past the threshold', async () => {
		const health = neverUp();
		health.reportOk();
		expect(health.outage).toBe('none');
		await goDown(health);
		expect(health.outage).toBe('reconnecting');
		await vi.advanceTimersByTimeAsync(RECONNECT_SCREEN_AFTER_MS - 1);
		expect(health.outage).toBe('reconnecting');
		await vi.advanceTimersByTimeAsync(1);
		expect(health.outage).toBe('offline');
		expect(health.nextRetryAt).not.toBeNull();
	});

	it('recovery clears the banner, resyncs once, and restarts the threshold', async () => {
		let answer = false;
		const health = new BackendHealth(vi.fn(async () => answer));
		const recovered = vi.fn();
		health.onRecover(recovered);
		health.reportOk();
		await goDown(health);
		await vi.advanceTimersByTimeAsync(RECONNECT_SCREEN_AFTER_MS);
		expect(health.outage).toBe('offline');

		answer = true;
		health.retryNow();
		await vi.advanceTimersByTimeAsync(0);
		expect(health.outage).toBe('none');
		expect(health.escalated).toBe(false);
		expect(recovered).toHaveBeenCalledTimes(1);

		answer = false;
		await vi.advanceTimersByTimeAsync(QUIET_MS);
		await goDown(health);
		expect(health.outage).toBe('reconnecting');
		await vi.advanceTimersByTimeAsync(RECONNECT_SCREEN_AFTER_MS - 1);
		expect(health.outage).toBe('reconnecting');
	});

	it('a good probe counts as contact even without a prior request', async () => {
		const { health } = healthWith([true, false]);
		health.reportFailure();
		await vi.advanceTimersByTimeAsync(DEBOUNCE_MS);
		expect(health.connected).toBe(true);
		await vi.advanceTimersByTimeAsync(QUIET_MS);
		await goDown(health);
		expect(health.outage).toBe('reconnecting');
	});
});
