import type { AgentEvent } from '@bindings/AgentEvent';
import { BoundedEventBuffer, KeyedListeners, turnIdOf, userMsgKey } from './frames';

export type StreamCb = (ev: AgentEvent) => void;

/** Per-session live event buffers and optimistic echoes, with their stream
 * listeners. Not reactive. */
export class SessionStreams {
	/** per-session event buffer (seed for late subscribers); not reactive */
	private buffer = new Map<string, BoundedEventBuffer>();
	/**
	 * Optimistic `reply` echoes the user just sent, kept here (NOT only in the
	 * component) so they survive a resubscribe/reconnect that rebuilds the
	 * drawer's local `live` from `bufferedEvents()`. A focus/reconnect-driven
	 * resub must not wipe them before the server echo arrives. Reconciled
	 * (dropped) once the server echoes the reply or the persisted `▷ User:`
	 * text form arrives. Not reactive.
	 */
	private optimistic = new Map<string, AgentEvent[]>();
	private streamCbs = new KeyedListeners<AgentEvent>();

	appendEvent(id: string, ev: AgentEvent) {
		// An incoming user-message event (server reply echo or the persisted
		// `▷ User:` text form) confirms an optimistic reply — drop it from the
		// pending store so it isn't re-seeded as a stale duplicate on resub.
		const turnId = turnIdOf(ev);
		const key = userMsgKey(ev);
		if (turnId !== null || key !== null) {
			const opt = this.optimistic.get(id);
			if (opt) {
				// Identity first; the content filter still runs when the echo
				// matched no optimistic entry by id, so an echo that lost the
				// turn id on the way back is still reconciled.
				let next = turnId !== null ? opt.filter((o) => turnIdOf(o) !== turnId) : opt;
				if (next.length === opt.length && key !== null) {
					next = opt.filter((o) => userMsgKey(o) !== key);
				}
				if (next.length !== opt.length) this.optimistic.set(id, next);
			}
		}
		// Defense-in-depth against duplicate live delivery: drop an
		// event whose full identity — including the daemon `ts` — already sits
		// in the buffer. A leaked/replayed duplicate carries the SAME daemon ts,
		// whereas a legitimately-repeated identical tool call within a turn gets
		// a DIFFERENT ts, so within-turn repeats are preserved.
		if (!this.bufFor(id).push(ev)) return;
		this.streamCbs.emit(id, ev);
	}

	bufFor(id: string): BoundedEventBuffer {
		let buf = this.buffer.get(id);
		if (!buf) {
			buf = new BoundedEventBuffer();
			this.buffer.set(id, buf);
		}
		return buf;
	}

	/** Record an optimistic reply the user just sent. Survives resubscribe and
	 * is reconciled away once the server echoes it back. */
	recordOptimistic(id: string, ev: AgentEvent) {
		this.optimistic.set(id, [...(this.optimistic.get(id) ?? []), ev]);
	}

	/** Drop a still-pending optimistic reply by its `ts`: used when
	 * the user edits a message that hasn't been acknowledged yet — the echo is
	 * pulled back into the composer, so it must stop being re-seeded on resub. */
	dropOptimistic(id: string, ts: number) {
		const opt = this.optimistic.get(id);
		if (opt) this.optimistic.set(id, opt.filter((o) => o.ts !== ts));
	}

	/** Snapshot of buffered events for a session (seed for a freshly-opened
	 * view), with any still-pending optimistic replies appended so a sent
	 * message survives a resubscribe until the server echoes it. */
	bufferedEvents(id: string): AgentEvent[] {
		return [...(this.buffer.get(id)?.list() ?? []), ...(this.optimistic.get(id) ?? [])];
	}

	/** Register a live-event listener for a session. Returns an unsubscribe fn. */
	onStream(id: string, cb: StreamCb): () => void {
		return this.streamCbs.add(id, cb);
	}
}
