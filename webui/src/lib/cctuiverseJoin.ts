import { ApiError } from './api';
import { joinInvite, type LinkView } from './cctuiverse';

/** Covers a cold spawn that the ack outran: agent start plus registration
 *  lands well inside a minute, and the invite (10 min, single use) is still
 *  valid when this gives up. */
export const JOIN_BUDGET_MS = 60_000;
const FIRST_DELAY_MS = 250;
const MAX_DELAY_MS = 4_000;

export function isSessionMissing(e: unknown): boolean {
	return e instanceof ApiError && e.status === 404 && e.message === 'no such session';
}

export interface JoinAfterSpawnOpts {
	probe: () => Promise<unknown>;
	budgetMs?: number;
	sleep?: (ms: number) => Promise<void>;
	now?: () => number;
}

export async function joinAfterSpawn(
	invite: string,
	sessionId: string,
	label: string,
	opts: JoinAfterSpawnOpts
): Promise<LinkView> {
	const now = opts.now ?? Date.now;
	const sleep = opts.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
	const deadline = now() + (opts.budgetMs ?? JOIN_BUDGET_MS);
	let delay = FIRST_DELAY_MS;
	let seen = false;
	for (;;) {
		if (!seen) seen = Boolean(await opts.probe().catch(() => null));
		if (seen || now() >= deadline) {
			try {
				return await joinInvite(invite, sessionId, label);
			} catch (e) {
				if (!isSessionMissing(e) || now() >= deadline) throw e;
			}
		}
		await sleep(delay);
		delay = Math.min(delay * 2, MAX_DELAY_MS);
	}
}
