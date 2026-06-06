import { browser } from '$app/environment';
import { wsBase } from './config';
import { auth } from './auth.svelte';
import type { AgentEvent } from '@bindings/AgentEvent';

export interface PermReq {
	session_id: string;
	request_id: string;
	tool_name: string;
	description: string;
	input_preview: string;
}

/** History stores the user's own turns as a `text` event prefixed with this
 * marker (there is no `reply` row on read); live optimistic echoes are `reply`
 * events. Shared so both shapes reconcile to one identity. */
export const USER_PREFIX = '▷ User:';

/** Stable identity of a user-typed message across its three shapes (optimistic
 * `reply`, server `reply` echo, persisted `▷ User:` text), or null if `ev`
 * isn't a user message. Used to reconcile optimistic echoes and to dedup the
 * live stream against fetched history. */
export function userMsgKey(ev: AgentEvent): string | null {
	if (ev.type === 'reply') return ev.content.trim();
	if (ev.type === 'text' && ev.content.startsWith(USER_PREFIX))
		return ev.content.slice(USER_PREFIX.length).trim();
	return null;
}

type Status = 'connecting' | 'open' | 'closed';
type StreamCb = (ev: AgentEvent) => void;
type PermCb = (list: PermReq[]) => void;
/**
 * A live AskUserQuestion. `question` is the flattened text (always present);
 * `questions` is the raw `tool_input.questions` array (header/options/
 * multiSelect) when the daemon's hook forwarded it (CCT-181), letting the
 * client render the interactive option-card form live instead of plain text.
 */
export interface LiveAsk {
	question: string;
	questions: unknown | null;
	/** Assistant prose preceding the question in the same turn, rendered above
	 * the card so the user has context instead of answering blind (CCT-213). */
	preamble?: string | null;
}
/** Live AskUserQuestion for a session, or null when none is pending. */
type AskCb = (ask: LiveAsk | null) => void;
/** Server ack for a client-sent message (CCT-212). `ok=false` means the server
 * could not dispatch the reply to the session's daemon, so the client should
 * mark the message failed and offer a retry. */
export interface MessageAck {
	client_msg_id: string;
	ok: boolean;
	error?: string;
}
/**
 * Per-session delivery state (CCT-214). A snapshot the drawer mirrors into
 * component-local `$state` (via `onDelivery`) so the red/Retry affordance and
 * the in-flight "sending…/retrying" tint render correctly — and, crucially,
 * SURVIVE the drawer being closed and reopened. The source of truth lives on
 * the singleton (not component `$state`), so a full unmount/remount no longer
 * drops a failed send's status the way it did before (the bubble used to come
 * back as a plain message with no Retry).
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

/** One tracked outbound message and its auto-retry lifecycle (CCT-214). */
interface TrackedSend {
	sid: string;
	/** the optimistic echo's `ts` — stable identity of the bubble in a session */
	ts: number;
	text: string;
	/** structured AskUserQuestion answer — per-question 0-based option picks
	 * (CCT-226). Carried on every retry so the daemon can drive the real form
	 * natively instead of dismissing it (which claude records as declined). */
	askPicks?: number[][];
	/** correlation id of the CURRENT attempt (rotates each retry) */
	clientMsgId: string;
	/** attempts dispatched so far (0 before the first dispatch) */
	attempt: number;
	phase: 'pending' | 'backoff' | 'failed';
	reason?: string;
	/** ack-timeout (pending) or backoff (backoff) handle */
	timer?: ReturnType<typeof setTimeout>;
}

// Auto-retry tuning (CCT-214). A dropped/failed send re-attempts with
// exponential backoff + jitter before giving up and going red; the user can
// always retry manually (which resets the counter).
const ACK_TIMEOUT_MS = 8000;
const MAX_ATTEMPTS = 5;
const BACKOFF_BASE_MS = 1000;
const BACKOFF_CAP_MS = 30000;
function backoffDelay(attempt: number): number {
	// attempt is 1-based (1 = first attempt just failed). Full jitter on top of
	// an exponential base, capped.
	const base = Math.min(BACKOFF_CAP_MS, BACKOFF_BASE_MS * 2 ** (attempt - 1));
	return Math.round(base * (0.75 + Math.random() * 0.5));
}

/**
 * Single shared TUI websocket. Streams live AgentEvents for subscribed
 * sessions, tracks pending permission requests, and resolves spawn command
 * results. Auto-reconnects with backoff.
 *
 * Live data is delivered to components via explicit per-session listener
 * callbacks (`onStream`/`onPerms`), NOT via reactive `$state` the component
 * reads back. Subscribers keep their own component-local `$state`, which is
 * the only reliable way to re-render — a `$derived`/effect that reads a
 * keyed `$state`/SvelteMap on this singleton from another module did NOT
 * re-run on mutation (the "chat window never refreshed live" bug). The ws
 * still keeps a small per-session buffer so a freshly-opened drawer can seed
 * from events that arrived before it registered.
 */
class WsClient {
	status = $state<Status>('closed');
	/** bumped whenever the session set/status changes, so lists can refetch */
	changeTick = $state(0);

	/** per-session event buffer (seed for late subscribers); not reactive */
	private buffer = new Map<string, AgentEvent[]>();
	/**
	 * Optimistic `reply` echoes the user just sent, kept here (NOT only in the
	 * component) so they survive a resubscribe/reconnect that rebuilds the
	 * drawer's local `live` from `bufferedEvents()`. Previously these lived only
	 * in component `$state` and a focus/reconnect-driven resub wiped them before
	 * the server echo arrived — the message claude received vanished from view.
	 * Reconciled (dropped) once the server echoes the reply or the persisted
	 * `▷ User:` text form arrives. Not reactive.
	 */
	private optimistic = new Map<string, AgentEvent[]>();
	/** pending permission prompts, keyed by session id; not reactive */
	private perms = new Map<string, PermReq[]>();
	/** pending AskUserQuestion, keyed by session id; not reactive (CCT-164) */
	private asks = new Map<string, LiveAsk>();
	private streamCbs = new Map<string, Set<StreamCb>>();
	private permCbs = new Map<string, Set<PermCb>>();
	private askCbs = new Map<string, Set<AskCb>>();
	/**
	 * Tracked outbound sends with their auto-retry state (CCT-214), keyed
	 * sid → ts. Lives here (not in the drawer) so a failed/in-flight send and
	 * its retry loop survive the drawer being closed and reopened. Not reactive
	 * — changes are pushed to subscribers via `deliveryCbs`.
	 */
	private sends = new Map<string, Map<number, TrackedSend>>();
	/** clientMsgId → the send it belongs to, for ack correlation (CCT-214). */
	private ackIndex = new Map<string, { sid: string; ts: number }>();
	private deliveryCbs = new Map<string, Set<DeliveryCb>>();

	private socket: WebSocket | null = null;
	private subscribed = new Set<string>();
	private waiters = new Map<
		string,
		(r: { ok: boolean; error?: string; timedOut?: boolean }) => void
	>();
	private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
	private listDirtyTimer: ReturnType<typeof setTimeout> | null = null;
	private want = false;

	connect() {
		if (!browser || !auth.token) return;
		this.want = true;
		this.open();
	}

	private open() {
		if (this.socket && this.socket.readyState <= WebSocket.OPEN) return;
		this.status = 'connecting';
		const url = `${wsBase()}/ws?token=${encodeURIComponent(auth.token)}`;
		const sock = new WebSocket(url);
		this.socket = sock;

		sock.onopen = () => {
			this.status = 'open';
			// re-subscribe everything after a reconnect
			for (const id of this.subscribed) this.send({ type: 'subscribe', session_id: id });
			// A send that failed because the socket was down is parked in `backoff`;
			// now that we're connected again, retry it immediately rather than
			// waiting out the timer (CCT-214).
			for (const m of this.sends.values()) {
				for (const s of m.values()) {
					if (s.phase === 'backoff') {
						this.clearTimer(s);
						this.dispatch(s);
					}
				}
			}
		};
		sock.onmessage = (ev) => this.onFrame(ev.data);
		sock.onclose = () => {
			this.status = 'closed';
			this.socket = null;
			if (this.want) this.scheduleReconnect();
		};
		sock.onerror = () => sock.close();
	}

	private scheduleReconnect() {
		if (this.reconnectTimer) return;
		this.reconnectTimer = setTimeout(() => {
			this.reconnectTimer = null;
			if (this.want) this.open();
		}, 3000);
	}

	disconnect() {
		this.want = false;
		this.socket?.close();
		this.socket = null;
		this.status = 'closed';
	}

	/** Write a frame to the socket. Returns true if it actually went out, false
	 * if the socket wasn't OPEN (frame dropped — callers must NOT treat a drop
	 * as a successful send). */
	private send(frame: Record<string, unknown>): boolean {
		if (this.socket?.readyState === WebSocket.OPEN) {
			this.socket.send(JSON.stringify(frame));
			return true;
		}
		return false;
	}

	private onFrame(raw: string) {
		let msg: Record<string, unknown>;
		try {
			msg = JSON.parse(raw);
		} catch {
			return;
		}
		switch (msg.type) {
			case 'stream': {
				const sid = msg.session_id as string;
				const data = msg.data as AgentEvent;
				this.appendEvent(sid, data);
				// Live events change a session's last-message/ordering in the list,
				// otherwise only refreshed on the 15s poll. Coalesce into one list
				// refresh so the list stays roughly live without refetching per event.
				this.markListDirty();
				break;
			}
			case 'permission_request': {
				const p = msg as unknown as PermReq;
				const list = this.perms.get(p.session_id) ?? [];
				if (!list.some((x) => x.request_id === p.request_id)) {
					this.setPerms(p.session_id, [...list, p]);
				}
				break;
			}
			case 'permission_resolved': {
				const sid = msg.session_id as string;
				const rid = msg.request_id as string;
				this.setPerms(
					sid,
					(this.perms.get(sid) ?? []).filter((x) => x.request_id !== rid)
				);
				break;
			}
			case 'ask_question': {
				const sid = msg.session_id as string;
				this.setAsk(sid, {
					question: msg.question as string,
					questions: (msg.questions as unknown) ?? null,
					preamble: (msg.preamble as string | undefined) ?? null
				});
				break;
			}
			case 'ask_resolved': {
				const sid = msg.session_id as string;
				this.setAsk(sid, null);
				break;
			}
			case 'message_ack': {
				const ack: MessageAck = {
					client_msg_id: msg.client_msg_id as string,
					ok: msg.ok as boolean,
					error: msg.error as string | undefined
				};
				// Resolve the tracked send (delivered / failed → auto-retry) (CCT-214).
				this.resolveAck(ack);
				break;
			}
			case 'command_result': {
				const cid = msg.command_id as string;
				const w = this.waiters.get(cid);
				if (w) {
					w({ ok: msg.ok as boolean, error: msg.error as string | undefined });
					this.waiters.delete(cid);
				}
				break;
			}
			case 'status':
			case 'session_registered':
			case 'session_deregistered':
				this.changeTick++;
				break;
		}
	}

	/** Debounced list-refresh trigger: bumps changeTick at most ~once/2s. */
	private markListDirty() {
		if (this.listDirtyTimer) return;
		this.listDirtyTimer = setTimeout(() => {
			this.listDirtyTimer = null;
			this.changeTick++;
		}, 2000);
	}

	private appendEvent(id: string, ev: AgentEvent) {
		// An incoming user-message event (server reply echo or the persisted
		// `▷ User:` text form) confirms an optimistic reply — drop it from the
		// pending store so it isn't re-seeded as a stale duplicate on resub.
		const key = userMsgKey(ev);
		if (key !== null) {
			const opt = this.optimistic.get(id);
			if (opt) {
				const next = opt.filter((o) => userMsgKey(o) !== key);
				if (next.length !== opt.length) this.optimistic.set(id, next);
			}
		}
		// Defense-in-depth against duplicate live delivery (CCT-182): drop an
		// event whose full identity — including the daemon `ts` — already sits
		// in the buffer. A leaked/replayed duplicate carries the SAME daemon ts,
		// whereas a legitimately-repeated identical tool call within a turn gets
		// a DIFFERENT ts, so within-turn repeats are preserved.
		const buf = this.buffer.get(id) ?? [];
		const sig = JSON.stringify(ev);
		if (buf.some((e) => JSON.stringify(e) === sig)) return;
		this.buffer.set(id, [...buf, ev]);
		const set = this.streamCbs.get(id);
		if (set) for (const cb of set) cb(ev);
	}

	/** Record an optimistic reply the user just sent. Survives resubscribe and
	 * is reconciled away once the server echoes it back. */
	recordOptimistic(id: string, ev: AgentEvent) {
		this.optimistic.set(id, [...(this.optimistic.get(id) ?? []), ev]);
	}

	/** Drop a still-pending optimistic reply by its `ts` (CCT-208): used when
	 * the user edits a message that hasn't been acknowledged yet — the echo is
	 * pulled back into the composer, so it must stop being re-seeded on resub. */
	dropOptimistic(id: string, ts: number) {
		const opt = this.optimistic.get(id);
		if (opt) this.optimistic.set(id, opt.filter((o) => o.ts !== ts));
	}

	private setPerms(id: string, list: PermReq[]) {
		this.perms.set(id, list);
		this.changeTick++; // list badge re-derives on changeTick-driven refetch
		const set = this.permCbs.get(id);
		if (set) for (const cb of set) cb(list);
	}

	private setAsk(id: string, ask: LiveAsk | null) {
		if (ask === null) this.asks.delete(id);
		else this.asks.set(id, ask);
		this.changeTick++;
		const set = this.askCbs.get(id);
		if (set) for (const cb of set) cb(ask);
	}

	subscribe(id: string) {
		if (!this.subscribed.has(id)) {
			this.subscribed.add(id);
			if (!this.buffer.has(id)) this.buffer.set(id, []);
			this.send({ type: 'subscribe', session_id: id });
		}
	}

	unsubscribe(id: string) {
		if (this.subscribed.delete(id)) {
			this.send({ type: 'unsubscribe', session_id: id });
		}
	}

	clearStream(id: string) {
		this.buffer.set(id, []);
	}

	/** Snapshot of buffered events for a session (seed for a freshly-opened
	 * view), with any still-pending optimistic replies appended so a sent
	 * message survives a resubscribe until the server echoes it. */
	bufferedEvents(id: string): AgentEvent[] {
		return [...(this.buffer.get(id) ?? []), ...(this.optimistic.get(id) ?? [])];
	}

	/** Current pending permission count for a session (read in list templates;
	 * the list re-derives on changeTick, which `setPerms` bumps). */
	pendingCount(id: string): number {
		return this.perms.get(id)?.length ?? 0;
	}

	/** Register a live-event listener for a session. Returns an unsubscribe fn. */
	onStream(id: string, cb: StreamCb): () => void {
		let set = this.streamCbs.get(id);
		if (!set) {
			set = new Set();
			this.streamCbs.set(id, set);
		}
		set.add(cb);
		return () => set!.delete(cb);
	}

	/** Register a pending-permissions listener for a session. Fires with the
	 * current list immediately and on every change. Returns an unsubscribe fn. */
	onPerms(id: string, cb: PermCb): () => void {
		let set = this.permCbs.get(id);
		if (!set) {
			set = new Set();
			this.permCbs.set(id, set);
		}
		set.add(cb);
		cb(this.perms.get(id) ?? []);
		return () => set!.delete(cb);
	}

	/** Register a live AskUserQuestion listener for a session. Fires with the
	 * current pending question (or null) immediately and on every change.
	 * Returns an unsubscribe fn (CCT-164). */
	onAsk(id: string, cb: AskCb): () => void {
		let set = this.askCbs.get(id);
		if (!set) {
			set = new Set();
			this.askCbs.set(id, set);
		}
		set.add(cb);
		cb(this.asks.get(id) ?? null);
		return () => set!.delete(cb);
	}

	/** Clear any live pending question for a session (e.g. after the user
	 * answers, before the daemon's resolution event arrives) (CCT-164). */
	clearAsk(id: string) {
		if (this.asks.has(id)) this.setAsk(id, null);
	}

	/** Send a typed message. Returns true if the frame went out, false if the
	 * socket wasn't OPEN (caller should keep the draft + surface a notice).
	 * `clientMsgId` (CCT-212) opts into a server `message_ack` so the caller can
	 * track delivery (sending → delivered / failed). */
	sendMessage(id: string, content: string, clientMsgId?: string, askPicks?: number[][]): boolean {
		return this.send({
			type: 'message',
			session_id: id,
			content,
			...(clientMsgId ? { client_msg_id: clientMsgId } : {}),
			...(askPicks ? { ask_picks: askPicks } : {})
		});
	}

	// ── Tracked send + auto-retry (CCT-214) ────────────────────────────────
	// The drawer creates the optimistic echo (it owns the `live` list + the
	// `ts` ordering) and hands us (sid, text, ts); we own the dispatch + ack
	// timeout + backoff retry loop, so the delivery state outlives the drawer.
	/** Begin tracking + dispatching a send. Returns whether the first frame
	 * actually left the socket (the caller uses this only for its optimistic
	 * working/ask UX — delivery itself is driven by acks + retries). */
	trackedSend(sid: string, text: string, ts: number, askPicks?: number[][]): boolean {
		let m = this.sends.get(sid);
		if (!m) {
			m = new Map();
			this.sends.set(sid, m);
		}
		const send: TrackedSend = { sid, ts, text, askPicks, clientMsgId: '', attempt: 0, phase: 'pending' };
		m.set(ts, send);
		return this.dispatch(send);
	}

	/** Manually retry a failed send — resets the attempt counter (CCT-214). */
	retryNow(sid: string, ts: number) {
		const send = this.sends.get(sid)?.get(ts);
		if (!send) return;
		this.clearTimer(send);
		send.attempt = 0;
		send.reason = undefined;
		this.dispatch(send);
	}

	/** Stop tracking a send (delivered, or pulled back into the composer to
	 * edit). Drops its timer + ack correlation (CCT-214). */
	cancelSend(sid: string, ts: number) {
		this.clearSend(sid, ts);
	}

	/** Forget all tracked sends for a session (e.g. on archive) (CCT-214). */
	clearDelivery(sid: string) {
		const m = this.sends.get(sid);
		if (m) {
			for (const s of m.values()) this.clearTimer(s);
			this.sends.delete(sid);
		}
		this.notifyDelivery(sid);
	}

	/** Current delivery snapshot for a session — seed for a freshly-(re)opened
	 * drawer; also pushed on every change via `onDelivery` (CCT-214). */
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
					if (s.phase === 'backoff') retrying.set(s.ts, { attempt: s.attempt, max: MAX_ATTEMPTS });
				}
			}
		}
		return { pending, retrying, failed };
	}

	/** Subscribe to a session's delivery state. Fires immediately with the
	 * current snapshot and on every change. Returns an unsubscribe fn (CCT-214). */
	onDelivery(sid: string, cb: DeliveryCb): () => void {
		let set = this.deliveryCbs.get(sid);
		if (!set) {
			set = new Set();
			this.deliveryCbs.set(sid, set);
		}
		set.add(cb);
		cb(this.deliverySnapshot(sid));
		return () => set!.delete(cb);
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
		const ok = this.sendMessage(send.sid, send.text, cid, send.askPicks);
		if (!ok) {
			// Socket wasn't OPEN — nudge a reconnect and park in backoff (onopen
			// re-dispatches) rather than burning straight to a hard failure.
			this.connect();
			this.onAttemptFailed(send, 'not connected — reconnecting');
			return false;
		}
		send.phase = 'pending';
		send.reason = undefined;
		send.timer = setTimeout(() => this.onAttemptFailed(send, 'no response from server'), ACK_TIMEOUT_MS);
		this.notifyDelivery(send.sid);
		return true;
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

	private resolveAck(ack: MessageAck) {
		const idx = this.ackIndex.get(ack.client_msg_id);
		if (!idx) return;
		this.ackIndex.delete(ack.client_msg_id);
		const send = this.sends.get(idx.sid)?.get(idx.ts);
		// Ignore a stale ack for a superseded attempt (a newer retry rotated the id).
		if (!send || send.clientMsgId !== ack.client_msg_id) return;
		if (ack.ok) this.clearSend(idx.sid, idx.ts);
		else this.onAttemptFailed(send, ack.error ?? 'could not deliver to the agent');
	}

	private clearSend(sid: string, ts: number) {
		const m = this.sends.get(sid);
		const send = m?.get(ts);
		if (send) {
			this.clearTimer(send);
			if (send.clientMsgId) this.ackIndex.delete(send.clientMsgId);
			m!.delete(ts);
		}
		this.notifyDelivery(sid);
	}

	private notifyDelivery(sid: string) {
		const set = this.deliveryCbs.get(sid);
		if (!set) return;
		const snap = this.deliverySnapshot(sid);
		for (const cb of set) cb(snap);
	}

	respondPermission(sessionId: string, requestId: string, allow: boolean) {
		this.send({
			type: 'permission_response',
			session_id: sessionId,
			request_id: requestId,
			behavior: allow ? 'allow' : 'deny'
		});
		this.setPerms(
			sessionId,
			(this.perms.get(sessionId) ?? []).filter((x) => x.request_id !== requestId)
		);
	}

	/** Resolve when the server reports a result for `commandId` (spawn).
	 *
	 * A timeout is NOT a failure (CCT-242): a cold spawn (kickstarting the
	 * agent daemon, staging uploads) can easily outlive any client-side wait,
	 * and the session still lands. `timedOut` lets the caller phrase it as
	 * "unconfirmed, check the list" instead of an error inviting a retry —
	 * re-submitting dispatches a brand-new spawn and a duplicate agent.
	 */
	awaitCommand(
		commandId: string,
		timeoutMs = 60_000
	): Promise<{ ok: boolean; error?: string; timedOut?: boolean }> {
		return new Promise((resolve) => {
			this.waiters.set(commandId, resolve);
			setTimeout(() => {
				if (this.waiters.delete(commandId)) {
					resolve({ ok: false, timedOut: true, error: 'no spawn confirmation from the daemon' });
				}
			}, timeoutMs);
		});
	}
}

export const ws = new WsClient();
