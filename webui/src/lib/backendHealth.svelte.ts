import { apiBase } from './config';

export type BackendState = 'up' | 'suspect' | 'down';

export function isOutageStatus(status: number): boolean {
	return status === 502 || status === 503 || status === 504;
}

export function isNetworkError(e: unknown): boolean {
	return e instanceof TypeError;
}

export const DEBOUNCE_MS = 1_500;
/** After a good probe, ignore failure signals for this long: a refused socket
 *  redialing every few seconds would otherwise probe forever. */
export const QUIET_MS = 5_000;
export const RETRY_BASE_MS = 1_000;
export const RETRY_MAX_MS = 30_000;

export function retryDelay(attempt: number): number {
	return Math.min(RETRY_MAX_MS, RETRY_BASE_MS * 2 ** attempt);
}

/** True when the server answered, whatever it said: only no answer or a
 *  gateway error means it is down. */
export type Probe = () => Promise<boolean>;

async function probeVersion(): Promise<boolean> {
	try {
		const res = await fetch(`${apiBase()}/version`, {
			cache: 'no-store',
			signal: AbortSignal.timeout(5_000)
		});
		return !isOutageStatus(res.status);
	} catch {
		return false;
	}
}

/** Failure signals only schedule a debounced `/version` probe; a failed probe
 *  declares the backend down, then it re-probes with backoff. */
export class BackendHealth {
	state = $state<BackendState>('up');
	nextRetryAt = $state<number | null>(null);
	probing = $state(false);

	private attempt = 0;
	private timer: ReturnType<typeof setTimeout> | null = null;
	private lastOkAt = Number.NEGATIVE_INFINITY;
	private waiters: Array<() => void> = [];
	private recoverHooks = new Set<() => void>();

	constructor(private probe: Probe = probeVersion) {}

	reportFailure() {
		if (this.state !== 'up') return;
		if (Date.now() - this.lastOkAt < QUIET_MS) return;
		this.suspect();
	}

	reportOk() {
		if (this.state !== 'up') this.markUp();
	}

	waitUntilUp(): Promise<void> {
		return new Promise((resolve) => {
			this.waiters.push(resolve);
			if (this.state === 'up') this.suspect();
		});
	}

	onRecover(fn: () => void): () => void {
		this.recoverHooks.add(fn);
		return () => this.recoverHooks.delete(fn);
	}

	retryNow() {
		if (this.state === 'down' && !this.probing) this.schedule(0);
	}

	private suspect() {
		this.state = 'suspect';
		this.schedule(DEBOUNCE_MS);
	}

	private schedule(ms: number) {
		if (this.timer) clearTimeout(this.timer);
		this.nextRetryAt = this.state === 'down' ? Date.now() + ms : null;
		this.timer = setTimeout(() => void this.runProbe(), ms);
	}

	private async runProbe() {
		this.timer = null;
		this.probing = true;
		let ok = false;
		try {
			ok = await this.probe();
		} catch {
			ok = false;
		}
		this.probing = false;
		if (this.state === 'up') return;
		if (ok) {
			this.markUp();
			return;
		}
		if (this.state !== 'down') {
			this.state = 'down';
			this.attempt = 0;
		}
		this.schedule(retryDelay(this.attempt++));
	}

	private markUp() {
		const wasDown = this.state === 'down';
		if (this.timer) clearTimeout(this.timer);
		this.timer = null;
		this.state = 'up';
		this.attempt = 0;
		this.nextRetryAt = null;
		this.lastOkAt = Date.now();
		for (const resolve of this.waiters.splice(0)) resolve();
		if (wasDown) for (const fn of this.recoverHooks) fn();
	}
}

export const backendHealth = new BackendHealth();
