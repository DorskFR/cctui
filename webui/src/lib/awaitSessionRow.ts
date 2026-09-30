import type { SessionListItem } from '@bindings/SessionListItem';

export interface AwaitSessionRowDeps {
	/** Already-loaded rows, checked first so the common case costs no request. */
	loaded: () => SessionListItem[];
	fetchOne: (id: string) => Promise<SessionListItem>;
	refetchLive: () => Promise<unknown> | unknown;
	sleep?: (ms: number) => Promise<void>;
	tries?: number;
	intervalMs?: number;
}

const wait = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

/**
 * A freshly spawned or forked session's DB row lags the spawn ack by a second
 * or three, so the id is real before anything can be found by it. Poll until
 * the row exists, or give up and let the caller say so.
 */
export async function awaitSessionRow(
	id: string,
	d: AwaitSessionRowDeps
): Promise<SessionListItem | null> {
	const sleep = d.sleep ?? wait;
	const tries = d.tries ?? 16;
	for (let i = 0; i < tries; i++) {
		const found = d.loaded().find((s) => s.id === id);
		if (found) return found;
		try {
			return await d.fetchOne(id);
		} catch {
			// Not registered yet.
		}
		await d.refetchLive();
		await sleep(d.intervalMs ?? 500);
	}
	return null;
}
