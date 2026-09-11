import { ghreviewUrl } from './config';
import { listGhreviewAccounts, type GhreviewAccount } from './ghreview';

/** `unset` — no ghreview deployment; `error` — the lookup failed, which is not
 *  the same answer as `none` and must never be reported as "you have none". */
export type ConnectorStatus = 'unknown' | 'loading' | 'unset' | 'none' | 'some' | 'error';

let status = $state<ConnectorStatus>('unknown');
let accounts = $state<GhreviewAccount[]>([]);
let inflight: Promise<void> | null = null;

export function connectorStatus(): ConnectorStatus {
	return status;
}

export function ghreviewAccounts(): GhreviewAccount[] {
	return accounts;
}

export function hasGithubConnector(): boolean {
	return status === 'some';
}

export function loadGhreviewConnectors(): Promise<void> {
	if (inflight) return inflight;
	if (ghreviewUrl() === null) {
		status = 'unset';
		accounts = [];
		return Promise.resolve();
	}
	if (status === 'some' || status === 'none') return Promise.resolve();

	status = 'loading';
	inflight = listGhreviewAccounts()
		.then((items) => {
			accounts = items;
			status = items.length > 0 ? 'some' : 'none';
		})
		.catch(() => {
			accounts = [];
			status = 'error';
		})
		.finally(() => {
			inflight = null;
		});
	return inflight;
}

export function invalidateGhreviewConnectors(): Promise<void> {
	status = 'unknown';
	accounts = [];
	inflight = null;
	return loadGhreviewConnectors();
}

export function resetGhreviewConnectors(): void {
	status = 'unknown';
	accounts = [];
	inflight = null;
}
