import type { AgentEvent } from '@bindings/AgentEvent';

export const CATCH_UP_TIMEOUT_MS = 10_000;

export interface CatchUpHost {
	cursor(sid: string): number | null;
	fetchAfter(sid: string, after: number): Promise<AgentEvent[]>;
	deliver(sid: string, ev: AgentEvent): void;
	recover(sid: string, after: number): void;
}

interface Gate {
	after: number;
	queued: AgentEvent[];
	timer: ReturnType<typeof setTimeout>;
}

const seqOf = (ev: AgentEvent): number | null => (ev.seq == null ? null : Number(ev.seq));

/** The relay only carries events from the new subscribe on, so a reconnect
 * fetches what was persisted past the cached tail and holds live events until
 * it lands, keeping the session's event order. */
export class StreamCatchUp {
	private gates = new Map<string, Gate>();

	constructor(
		private host: CatchUpHost,
		private timeoutMs = CATCH_UP_TIMEOUT_MS
	) {}

	begin(sid: string) {
		const prev = this.gates.get(sid);
		const after = prev ? prev.after : this.host.cursor(sid);
		if (after === null) return;
		if (prev) clearTimeout(prev.timer);
		if (after <= 0) {
			this.gates.delete(sid);
			for (const ev of prev?.queued ?? []) this.host.deliver(sid, ev);
			this.host.recover(sid, 0);
			return;
		}
		const gate: Gate = {
			after,
			queued: prev?.queued ?? [],
			timer: setTimeout(() => this.fail(sid, gate), this.timeoutMs)
		};
		this.gates.set(sid, gate);
		this.host.fetchAfter(sid, after).then(
			(evs) => this.land(sid, gate, evs),
			() => this.fail(sid, gate)
		);
	}

	hold(sid: string, ev: AgentEvent): boolean {
		const gate = this.gates.get(sid);
		if (!gate) return false;
		gate.queued.push(ev);
		return true;
	}

	cancel(sid: string) {
		const gate = this.gates.get(sid);
		if (!gate) return;
		clearTimeout(gate.timer);
		this.gates.delete(sid);
	}

	private take(sid: string, gate: Gate): boolean {
		if (this.gates.get(sid) !== gate) return false;
		clearTimeout(gate.timer);
		this.gates.delete(sid);
		return true;
	}

	private land(sid: string, gate: Gate, fetched: AgentEvent[]) {
		if (!this.take(sid, gate)) return;
		const seen = new Set<number>();
		const missed = fetched
			.filter((ev) => (seqOf(ev) ?? 0) > gate.after)
			.sort((a, b) => (seqOf(a) ?? 0) - (seqOf(b) ?? 0));
		for (const ev of missed) {
			const seq = seqOf(ev);
			if (seq === null || seen.has(seq)) continue;
			seen.add(seq);
			this.host.deliver(sid, ev);
		}
		for (const ev of gate.queued) {
			const seq = seqOf(ev);
			if (seq === null || !seen.has(seq)) this.host.deliver(sid, ev);
		}
	}

	private fail(sid: string, gate: Gate) {
		if (!this.take(sid, gate)) return;
		for (const ev of gate.queued) this.host.deliver(sid, ev);
		this.host.recover(sid, gate.after);
	}
}
