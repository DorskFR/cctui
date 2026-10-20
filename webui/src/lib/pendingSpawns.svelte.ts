import type { SessionListItem } from '@bindings/SessionListItem';
import { findSpawnedSession } from '$lib/components/organisms/spawn/spawnMatch';
import { ws } from '$lib/ws.svelte';

/** A spawn the daemon acknowledged but that had not registered a session when
 *  the form stopped waiting. It stays on the list as a card until the session
 *  appears, the daemon reports the failure, or the user dismisses it. */
export type PendingSpawn = {
	commandId: string;
	sessionId: string | null;
	machineId: string;
	name: string;
	workingDir: string;
	since: number;
	failure: string | null;
};

class PendingSpawns {
	items = $state<PendingSpawn[]>([]);
	#wired = false;

	add(entry: Omit<PendingSpawn, 'since' | 'failure'>) {
		this.wire();
		if (this.items.some((p) => p.commandId === entry.commandId)) return;
		this.items = [...this.items, { ...entry, since: Date.now(), failure: null }];
	}

	/** The daemon's late verdict: a failure turns the card red, a success means
	 *  the session is about to appear and the card has done its job. */
	resolve(commandId: string, ok: boolean, error?: string | null) {
		const p = this.items.find((x) => x.commandId === commandId);
		if (!p) return;
		if (ok) this.dismiss(commandId);
		else p.failure = error?.trim() || 'spawn failed';
	}

	/** A session ended before the card was settled: `spawn_failed` is the
	 *  swept failure, anything else means it had landed. */
	ended(sessionId: string, reason: string | null, detail: string | null) {
		const p = this.items.find((x) => x.sessionId === sessionId);
		if (!p) return;
		if (reason === 'spawn_failed' || reason === 'resume_failed') p.failure = detail?.trim() || reason;
		else this.dismiss(p.commandId);
	}

	/** Codex and OpenCode choose their id after dispatch: learn it from the
	 *  listed session's spawn key. */
	adopt(sessions: Pick<SessionListItem, 'id' | 'machine_id' | 'metadata'>[]) {
		for (const p of this.items) {
			if (p.sessionId) continue;
			const hit = findSpawnedSession(sessions, p.machineId, p.commandId, null);
			if (hit) p.sessionId = hit.id;
		}
	}

	/** Drop every pending card whose session now exists on the list. */
	settle(knownSessionIds: Iterable<string>) {
		const known = new Set(knownSessionIds);
		const rest = this.items.filter((p) => p.failure !== null || !p.sessionId || !known.has(p.sessionId));
		if (rest.length !== this.items.length) this.items = rest;
	}

	dismiss(commandId: string) {
		this.items = this.items.filter((p) => p.commandId !== commandId);
	}

	/** Cards whose session is not on the list yet, failures included. */
	visible(knownSessionIds: Iterable<string>): PendingSpawn[] {
		const known = new Set(knownSessionIds);
		return this.items.filter((p) => p.failure !== null || !p.sessionId || !known.has(p.sessionId));
	}

	private wire() {
		if (this.#wired) return;
		this.#wired = true;
		ws.onCommandResult((r) => this.resolve(r.commandId, r.ok, r.error));
		ws.onSessionEnded((ev) => this.ended(ev.session_id, ev.reason, ev.detail ?? null));
	}

	/** Tests only: forget every card. */
	reset() {
		this.items = [];
	}
}

export const pendingSpawns = new PendingSpawns();
