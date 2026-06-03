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
type MessageAckCb = (ack: MessageAck) => void;

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
	private msgAckCbs = new Map<string, Set<MessageAckCb>>();

	private socket: WebSocket | null = null;
	private subscribed = new Set<string>();
	private waiters = new Map<string, (r: { ok: boolean; error?: string }) => void>();
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
					questions: (msg.questions as unknown) ?? null
				});
				break;
			}
			case 'ask_resolved': {
				const sid = msg.session_id as string;
				this.setAsk(sid, null);
				break;
			}
			case 'message_ack': {
				const sid = msg.session_id as string;
				const ack: MessageAck = {
					client_msg_id: msg.client_msg_id as string,
					ok: msg.ok as boolean,
					error: msg.error as string | undefined
				};
				const set = this.msgAckCbs.get(sid);
				if (set) for (const cb of set) cb(ack);
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
	sendMessage(id: string, content: string, clientMsgId?: string): boolean {
		return this.send({
			type: 'message',
			session_id: id,
			content,
			...(clientMsgId ? { client_msg_id: clientMsgId } : {})
		});
	}

	/** Register a message-ack listener for a session (CCT-212). Returns an
	 * unsubscribe fn. */
	onMessageAck(id: string, cb: MessageAckCb): () => void {
		let set = this.msgAckCbs.get(id);
		if (!set) {
			set = new Set();
			this.msgAckCbs.set(id, set);
		}
		set.add(cb);
		return () => set!.delete(cb);
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

	/** Resolve when the server reports a result for `commandId` (spawn). */
	awaitCommand(commandId: string, timeoutMs = 20_000): Promise<{ ok: boolean; error?: string }> {
		return new Promise((resolve) => {
			this.waiters.set(commandId, resolve);
			setTimeout(() => {
				if (this.waiters.delete(commandId)) {
					resolve({ ok: false, error: 'timed out waiting for daemon ACK' });
				}
			}, timeoutMs);
		});
	}
}

export const ws = new WsClient();
