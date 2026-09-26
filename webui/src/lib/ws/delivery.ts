import { KeyedListeners, type CommandOutcome, type MessageAck } from './frames';

/** How long to wait for the adapter's delivery result after the server acked
 *  the dispatch. Adapters that never report one leave the send unconfirmed
 *  rather than failed, so a slow agent never invites a duplicate send. */
const DELIVERY_ACK_TIMEOUT_MS = 20_000;
/**
 * Per-session delivery state. A snapshot the drawer mirrors into
 * component-local `$state` (via `onDelivery`) so the red/Retry affordance and
 * the in-flight "sending…/retrying" tint render correctly — and, crucially,
 * SURVIVE the drawer being closed and reopened. The source of truth lives on
 * the singleton (not component `$state`), so a full unmount/remount does not
 * drop a failed send's status.
 * - `pending`: ts of sends in flight (awaiting ack) or waiting on a backoff.
 * - `retrying`: ts → auto-retry progress, for a "retrying (n/m)" hint.
 * - `failed`: ts → reason, for sends that exhausted auto-retry (red + Retry).
 */
export interface DeliverySnapshot {
	pending: Set<number>;
	retrying: Map<number, { attempt: number; max: number }>;
	failed: Map<number, string>;
}
type DeliveryCb = (snap: DeliverySnapshot) => void;

/** One tracked outbound message and its auto-retry lifecycle. */
interface TrackedSend {
	sid: string;
	/** the optimistic echo's `ts` — stable identity of the bubble in a session */
	ts: number;
	text: string;
	/** structured AskUserQuestion answer — per-question 0-based option picks.
	 * Carried on every retry so the daemon can drive the real form
	 * natively instead of dismissing it (which claude records as declined). */
	askPicks?: number[][];
	/** the turn's identity, re-sent on every retry so a retried frame the
	 * server did receive produces the same turn id as the first attempt */
	turnId?: string;
	/** correlation id of the CURRENT attempt (rotates each retry) */
	clientMsgId: string;
	/** attempts dispatched so far (0 before the first dispatch) */
	attempt: number;
	phase: 'pending' | 'backoff' | 'failed';
	reason?: string;
	/** ack-timeout (pending) or backoff (backoff) handle */
	timer?: ReturnType<typeof setTimeout>;
}

// Auto-retry tuning. A dropped/failed send re-attempts with
// exponential backoff + jitter before giving up and going red; the user can
// always retry manually (which resets the counter).
const ACK_TIMEOUT_MS = 8000;
const MAX_ATTEMPTS = 5;

/** Re-dispatch delay for a send parked on a missing socket. Off the backoff
 *  ladder: it is waiting for a transport, not backing off a server. */
const RECONNECT_PARK_MS = 1000;

const BACKOFF_BASE_MS = 1000;
const BACKOFF_CAP_MS = 30000;
export function backoffDelay(attempt: number): number {
	// attempt is 1-based (1 = first attempt just failed). Full jitter on top of
	// an exponential base, capped.
	const base = Math.min(BACKOFF_CAP_MS, BACKOFF_BASE_MS * 2 ** (attempt - 1));
	return Math.round(base * (0.75 + Math.random() * 0.5));
}

/** What the tracker needs from the socket it sends through. */
export interface DeliveryHost {
	sendMessage(
		id: string,
		content: string,
		clientMsgId?: string,
		askPicks?: number[][],
		turnId?: string
	): boolean;
	connect(): void;
	forceReconnect(): void;
	awaitCommand(commandId: string, timeoutMs?: number): Promise<CommandOutcome>;
}

/** Tracked outbound sends and their ack/auto-retry lifecycle. */
export class DeliveryTracker {
	/**
	 * Tracked outbound sends with their auto-retry state, keyed
	 * sid → ts. Lives here (not in the drawer) so a failed/in-flight send and
	 * its retry loop survive the drawer being closed and reopened. Not reactive
	 * — changes are pushed to subscribers via `deliveryCbs`.
	 */
	private sends = new Map<string, Map<number, TrackedSend>>();
	/** clientMsgId → the send it belongs to, for ack correlation. */
	private ackIndex = new Map<string, { sid: string; ts: number }>();
	private deliveryCbs = new KeyedListeners<DeliverySnapshot>();

	constructor(private host: DeliveryHost) {}

	// ── Tracked send + auto-retry ────────────────────────────────
	// The drawer creates the optimistic echo (it owns the `live` list + the
	// `ts` ordering) and hands us (sid, text, ts); we own the dispatch + ack
	// timeout + backoff retry loop, so the delivery state outlives the drawer.
	/** Begin tracking + dispatching a send. Returns whether the first frame
	 * actually left the socket (the caller uses this only for its optimistic
	 * working/ask UX — delivery itself is driven by acks + retries). */
	trackedSend(
		sid: string,
		text: string,
		ts: number,
		askPicks?: number[][],
		turnId?: string
	): boolean {
		let m = this.sends.get(sid);
		if (!m) {
			m = new Map();
			this.sends.set(sid, m);
		}
		const send: TrackedSend = {
			sid,
			ts,
			text,
			askPicks,
			turnId,
			clientMsgId: '',
			attempt: 0,
			phase: 'pending'
		};
		m.set(ts, send);
		return this.dispatch(send);
	}

	/** Manually retry a failed send — resets the attempt counter. */
	retryNow(sid: string, ts: number) {
		const send = this.sends.get(sid)?.get(ts);
		if (!send) return;
		this.clearTimer(send);
		send.attempt = 0;
		send.reason = undefined;
		this.dispatch(send);
	}

	/** Stop tracking a send (delivered, or pulled back into the composer to
	 * edit). Drops its timer + ack correlation. */
	cancelSend(sid: string, ts: number) {
		this.clearSend(sid, ts);
	}

	/** Forget all tracked sends for a session (e.g. on archive). */
	clearDelivery(sid: string) {
		const m = this.sends.get(sid);
		if (m) {
			for (const s of m.values()) this.clearTimer(s);
			this.sends.delete(sid);
		}
		this.notifyDelivery(sid);
	}

	/** Current delivery snapshot for a session — seed for a freshly-(re)opened
	 * drawer; also pushed on every change via `onDelivery`. */
	deliverySnapshot(sid: string): DeliverySnapshot {
		const pending = new Set<number>();
		const retrying = new Map<number, { attempt: number; max: number }>();
		const failed = new Map<number, string>();
		const m = this.sends.get(sid);
		if (m) {
			for (const s of m.values()) {
				if (s.phase === 'failed') {
					failed.set(s.ts, s.reason ?? 'not delivered');
				} else {
					pending.add(s.ts);
					if (s.phase === 'backoff')
						// A send parked before its first successful write sits at
						// attempt 0; the hint counts from 1.
						retrying.set(s.ts, { attempt: Math.max(1, s.attempt), max: MAX_ATTEMPTS });
				}
			}
		}
		return { pending, retrying, failed };
	}

	/** Subscribe to a session's delivery state. Fires immediately with the
	 * current snapshot and on every change. Returns an unsubscribe fn. */
	onDelivery(sid: string, cb: DeliveryCb): () => void {
		const off = this.deliveryCbs.add(sid, cb);
		cb(this.deliverySnapshot(sid));
		return off;
	}

	/** A send that failed because the socket was down is parked in `backoff`;
	 * once connected again, retry it immediately rather than waiting out the
	 * timer. */
	redispatchParked() {
		for (const m of this.sends.values()) {
			for (const s of m.values()) {
				if (s.phase === 'backoff') {
					this.clearTimer(s);
					this.dispatch(s);
				}
			}
		}
	}

	private clearTimer(send: TrackedSend) {
		if (send.timer) {
			clearTimeout(send.timer);
			send.timer = undefined;
		}
	}

	/** Send one attempt: rotate the correlation id, write the frame, and arm an
	 * ack timeout. A dropped frame (socket down) schedules a backoff retry. */
	private dispatch(send: TrackedSend): boolean {
		this.clearTimer(send);
		send.attempt += 1;
		const cid =
			typeof crypto !== 'undefined' && crypto.randomUUID
				? crypto.randomUUID()
				: `${send.ts}-${send.attempt}`;
		// Drop the previous attempt's correlation so a late, superseded ack is ignored.
		if (send.clientMsgId) this.ackIndex.delete(send.clientMsgId);
		send.clientMsgId = cid;
		this.ackIndex.set(cid, { sid: send.sid, ts: send.ts });
		const ok = this.host.sendMessage(send.sid, send.text, cid, send.askPicks, send.turnId);
		if (!ok) {
			// A frame that never left the client is not a delivery attempt: the
			// budget measures sends the server ignored. Counting failed writes
			// would spend MAX_ATTEMPTS on a slow reconnect and turn "offline"
			// into a permanently red message.
			send.attempt -= 1;
			this.host.connect();
			this.parkForReconnect(send);
			return false;
		}
		send.phase = 'pending';
		send.reason = undefined;
		send.timer = setTimeout(() => this.onAckTimeout(send), ACK_TIMEOUT_MS);
		this.notifyDelivery(send.sid);
		return true;
	}

	/** A frame that left an `OPEN` socket and drew no ack is the signature of a
	 * half-open connection, so the retry needs a new socket — every further
	 * attempt on this one would time out identically. Park the send first, so
	 * the new socket's `onopen` finds it and re-dispatches. */
	private onAckTimeout(send: TrackedSend) {
		this.onAttemptFailed(send, 'no response from server');
		this.host.forceReconnect();
	}

	/** Hold a send until the socket is back without spending retry budget.
	 *  `onopen` re-dispatches it; the timer is only a backstop for a reconnect
	 *  that never completes. */
	private parkForReconnect(send: TrackedSend) {
		this.clearTimer(send);
		send.phase = 'backoff';
		send.reason = 'not connected — reconnecting';
		send.timer = setTimeout(() => this.dispatch(send), RECONNECT_PARK_MS);
		this.notifyDelivery(send.sid);
	}

	/** An attempt failed (bad ack, ack timeout, or dropped frame): schedule a
	 * backoff retry, or give up (red + manual Retry) once attempts are spent. */
	private onAttemptFailed(send: TrackedSend, reason: string) {
		this.clearTimer(send);
		if (send.attempt >= MAX_ATTEMPTS) {
			send.phase = 'failed';
			send.reason = reason;
			this.notifyDelivery(send.sid);
			return;
		}
		send.phase = 'backoff';
		send.reason = reason;
		send.timer = setTimeout(() => this.dispatch(send), backoffDelay(send.attempt));
		this.notifyDelivery(send.sid);
	}

	resolveAck(ack: MessageAck) {
		const idx = this.ackIndex.get(ack.client_msg_id);
		if (!idx) return;
		this.ackIndex.delete(ack.client_msg_id);
		const send = this.sends.get(idx.sid)?.get(idx.ts);
		// Ignore a stale ack for a superseded attempt (a newer retry rotated the id).
		if (!send || send.clientMsgId !== ack.client_msg_id) return;
		if (!ack.ok) {
			this.onAttemptFailed(send, ack.error ?? 'could not deliver to the agent');
			return;
		}
		if (!ack.command_id) {
			this.clearSend(idx.sid, idx.ts);
			return;
		}
		// An ok ack only means the frame was queued toward a daemon. Hold the
		// send until the adapter reports it actually delivered, so a reply that
		// reached the wrong daemon goes red instead of vanishing.
		this.clearTimer(send);
		void this.host.awaitCommand(ack.command_id, DELIVERY_ACK_TIMEOUT_MS).then((res) => {
			const current = this.sends.get(idx.sid)?.get(idx.ts);
			if (!current || current.clientMsgId !== ack.client_msg_id) return;
			if (res.ok || res.timedOut) this.clearSend(idx.sid, idx.ts);
			else this.onAttemptFailed(current, res.error ?? 'the agent did not accept the message');
		});
	}

	private clearSend(sid: string, ts: number) {
		const m = this.sends.get(sid);
		const send = m?.get(ts);
		if (send) {
			this.clearTimer(send);
			if (send.clientMsgId) this.ackIndex.delete(send.clientMsgId);
			m?.delete(ts);
		}
		this.notifyDelivery(sid);
	}

	private notifyDelivery(sid: string) {
		if (!this.deliveryCbs.has(sid)) return;
		this.deliveryCbs.emit(sid, this.deliverySnapshot(sid));
	}
}
