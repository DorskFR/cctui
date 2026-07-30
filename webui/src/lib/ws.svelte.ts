import { browser } from '$app/environment';
import { wsBase } from './config';
import { auth } from './auth.svelte';
import type { AgentEvent } from '@bindings/AgentEvent';
import type { GithubEventKind } from '@bindings/GithubEventKind';
import type { GithubEventPayload } from '@bindings/GithubEventPayload';

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

/** Canonicalize a user-turn body so its optimistic-echo and persisted/server
 * shapes reduce to the same string. The composer appends staged
 * *absolute* upload paths under an `Attached file(s):` header, and the
 * persisted form can diverge on whitespace, trailing newlines, and path
 * rewrites — so content-based dedup fails for attachment messages. Normalize
 * defensively: trim each line, drop blank lines, and reduce every `- <path>`
 * bullet to its basename so absolute-path differences don't matter. */
export function canonUserBody(body: string): string {
	return body
		.split('\n')
		.map((line) => {
			const trimmed = line.trim();
			// Attachment bullets: `- /abs/path/to/file` → `- file`. Also collapses
			// emptied bullets (`-`) the persisted form sometimes carries.
			const m = trimmed.match(/^-\s*(.*)$/);
			if (m) {
				const path = m[1].trim();
				const base = path.split(/[\\/]/).pop() ?? '';
				return base ? `- ${base}` : '-';
			}
			return trimmed;
		})
		.filter((line) => line.length > 0)
		.join('\n');
}

/** Stable identity of a user-typed message across its three shapes (optimistic
 * `reply`, server `reply` echo, persisted `▷ User:` text), or null if `ev`
 * isn't a user message. Used to reconcile optimistic echoes and to dedup the
 * live stream against fetched history. */
export function userMsgKey(ev: AgentEvent): string | null {
	if (ev.type === 'reply') return canonUserBody(ev.content);
	if (ev.type === 'text' && ev.content.startsWith(USER_PREFIX))
		return canonUserBody(ev.content.slice(USER_PREFIX.length));
	return null;
}

type Status = 'connecting' | 'open' | 'closed';
type StreamCb = (ev: AgentEvent) => void;
type PtyCb = (data: Uint8Array) => void;
type PermCb = (list: PermReq[]) => void;
/** A live GitHub inbox nudge (GH-CONN-5): "something about a tracked PR
 * changed" — the `/github` inbox refetches the affected rows in response. */
export interface GithubEvent {
	kind: GithubEventKind;
	payload: GithubEventPayload;
}
type GithubCb = (ev: GithubEvent) => void;
/**
 * A live AskUserQuestion. `question` is the flattened text (always present);
 * `questions` is the raw `tool_input.questions` array (header/options/
 * multiSelect) when the daemon's hook forwarded it, letting the
 * client render the interactive option-card form live instead of plain text.
 */
export interface LiveAsk {
	question: string;
	questions: unknown | null;
	/** Assistant prose preceding the question in the same turn, rendered above
	 * the card so the user has context instead of answering blind. */
	preamble?: string | null;
}
/** Live AskUserQuestion for a session, or null when none is pending. */
type AskCb = (ask: LiveAsk | null) => void;
/**
 * A live ExitPlanMode plan-approval prompt. `plan` is the plan
 * markdown the agent presented; `preamble` is the prose preceding the
 * `ExitPlanMode` call, rendered above the Plan card for context.
 */
export interface LivePlan {
	plan: string;
	preamble?: string | null;
}
/** Live plan prompt for a session, or null when none is pending. */
type PlanCb = (plan: LivePlan | null) => void;
/**
 * A session's per-account soft-limit block. The gateway refused a
 * request because cctui's own share of `account_name`'s usage window is at cap;
 * the conversation stalled with a 429. The webui surfaces a per-chat banner
 * offering to continue on another same-provider account.
 */
export interface SoftLimit {
	account_id: string;
	account_name: string;
	reason: string;
	retry_after_secs: number;
}
/** Live soft-limit block for a session, or null when none is active. */
type SoftLimitCb = (sl: SoftLimit | null) => void;
/** Server ack for a client-sent message. `ok=false` means the server
 * could not dispatch the reply to the session's daemon, so the client should
 * mark the message failed and offer a retry. */
export interface MessageAck {
	client_msg_id: string;
	ok: boolean;
	error?: string;
}
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
/** Decode a standard-base64 string to raw bytes (PTY chunks). */
export function decodeBase64(b64: string): Uint8Array {
	const bin = atob(b64);
	const out = new Uint8Array(bin.length);
	for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
	return out;
}

const BACKOFF_BASE_MS = 1000;
const BACKOFF_CAP_MS = 30000;
function backoffDelay(attempt: number): number {
	// attempt is 1-based (1 = first attempt just failed). Full jitter on top of
	// an exponential base, capped.
	const base = Math.min(BACKOFF_CAP_MS, BACKOFF_BASE_MS * 2 ** (attempt - 1));
	return Math.round(base * (0.75 + Math.random() * 0.5));
}

/**
 * A registry of per-key callback sets. `add` returns an unsubscribe fn and drops
 * the key's set once empty, so a long-lived client doesn't accumulate one entry
 * per session ever visited.
 */
export class KeyedListeners<T> {
	private cbs = new Map<string, Set<(v: T) => void>>();

	add(key: string, cb: (v: T) => void): () => void {
		let set = this.cbs.get(key);
		if (!set) {
			set = new Set();
			this.cbs.set(key, set);
		}
		set.add(cb);
		return () => {
			const cur = this.cbs.get(key);
			if (!cur) return;
			cur.delete(cb);
			if (cur.size === 0) this.cbs.delete(key);
		};
	}

	emit(key: string, value: T) {
		const set = this.cbs.get(key);
		if (set) for (const cb of set) cb(value);
	}

	has(key: string): boolean {
		return (this.cbs.get(key)?.size ?? 0) > 0;
	}
}

/** Per-session seed buffer caps. A subscribed-but-unopened session streams
 * events forever, so the buffer is bounded on both counts — it only ever needs
 * to seed a freshly-opened drawer, which then fetches real history. */
const MAX_BUFFER_CHARS = 1_000_000;
const MAX_BUFFER_EVENTS = 1500;

/**
 * Bounded, de-duplicating event buffer. Dedup is by serialized identity via a
 * hash set (one serialization per arriving event), and the total serialized size
 * is tracked incrementally — so appending is O(1), not O(N) stringifies.
 */
export class BoundedEventBuffer {
	private entries: { ev: AgentEvent; sig: string }[] = [];
	private sigs = new Set<string>();
	private chars = 0;

	constructor(
		private maxChars = MAX_BUFFER_CHARS,
		private maxEvents = MAX_BUFFER_EVENTS
	) {}

	push(ev: AgentEvent): boolean {
		const sig = JSON.stringify(ev);
		if (this.sigs.has(sig)) return false;
		this.entries.push({ ev, sig });
		this.sigs.add(sig);
		this.chars += sig.length;
		// Evict oldest-first, but never the event just appended.
		while (this.entries.length > 1 && (this.entries.length > this.maxEvents || this.chars > this.maxChars)) {
			const dropped = this.entries.shift();
			if (!dropped) break;
			this.sigs.delete(dropped.sig);
			this.chars -= dropped.sig.length;
		}
		return true;
	}

	list(): AgentEvent[] {
		return this.entries.map((e) => e.ev);
	}

	clear() {
		this.entries = [];
		this.sigs.clear();
		this.chars = 0;
	}

	get size(): number {
		return this.entries.length;
	}
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
	private buffer = new Map<string, BoundedEventBuffer>();
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
	/** pending AskUserQuestion, keyed by session id; not reactive */
	private asks = new Map<string, LiveAsk>();
	private plans = new Map<string, LivePlan>();
	private softLimits = new Map<string, SoftLimit>();
	private streamCbs = new KeyedListeners<AgentEvent>();
	/** Live PTY-view listeners keyed by session id; not reactive. The
	 * bytes are never buffered — a terminal that mounts late relies on the fresh
	 * attach's full-screen repaint, not replay. */
	private ptyCbs = new KeyedListeners<Uint8Array>();
	/** Sessions this client is watching the live terminal of, re-sent on
	 * reconnect so the daemon stream resumes after a drop. */
	private ptyWatched = new Set<string>();
	private permCbs = new KeyedListeners<PermReq[]>();
	private askCbs = new KeyedListeners<LiveAsk | null>();
	private planCbs = new KeyedListeners<LivePlan | null>();
	private softLimitCbs = new KeyedListeners<SoftLimit | null>();
	/** GitHub inbox listeners (GH-CONN-5 / GH-UI-1); not session-keyed — one
	 * broadcast channel the mounted inbox subscribes to. Not reactive. */
	private githubCbs = new Set<GithubCb>();
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
		if (!browser || !auth.isAuthed) return;
		this.want = true;
		this.open();
	}

	private open() {
		if (this.socket && this.socket.readyState <= WebSocket.OPEN) return;
		this.status = 'connecting';
		// Same-origin WS upgrade: the browser attaches the `HttpOnly` auth cookie
		// automatically, so the token no longer rides the query string.
		const url = `${wsBase()}/ws`;
		const sock = new WebSocket(url);
		this.socket = sock;

		sock.onopen = () => {
			this.status = 'open';
			// re-subscribe everything after a reconnect
			for (const id of this.subscribed) this.send({ type: 'subscribe', session_id: id });
			// re-arm live-terminal watches so the daemon PTY stream resumes
			for (const id of this.ptyWatched)
				this.send({ type: 'watch_terminal', session_id: id, watch: true });
			// A send that failed because the socket was down is parked in `backoff`;
			// now that we're connected again, retry it immediately rather than
			// waiting out the timer.
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
			case 'pty_chunk': {
				const sid = msg.session_id as string;
				if (this.ptyCbs.has(sid)) this.ptyCbs.emit(sid, decodeBase64(msg.data as string));
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
			case 'plan_request': {
				const sid = msg.session_id as string;
				this.setPlan(sid, {
					plan: msg.plan as string,
					preamble: (msg.preamble as string | undefined) ?? null
				});
				break;
			}
			case 'plan_resolved': {
				const sid = msg.session_id as string;
				this.setPlan(sid, null);
				break;
			}
			case 'soft_limit_reached': {
				const sid = msg.session_id as string;
				this.setSoftLimit(sid, {
					account_id: msg.account_id as string,
					account_name: msg.account_name as string,
					reason: msg.reason as string,
					retry_after_secs: msg.retry_after_secs as number
				});
				break;
			}
			case 'soft_limit_cleared': {
				const sid = msg.session_id as string;
				this.setSoftLimit(sid, null);
				break;
			}
			case 'message_ack': {
				const ack: MessageAck = {
					client_msg_id: msg.client_msg_id as string,
					ok: msg.ok as boolean,
					error: msg.error as string | undefined
				};
				// Resolve the tracked send (delivered / failed → auto-retry).
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
			case 'github_event': {
				const ev: GithubEvent = {
					kind: msg.kind as GithubEventKind,
					payload: msg.payload as GithubEventPayload
				};
				for (const cb of this.githubCbs) cb(ev);
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
		// Defense-in-depth against duplicate live delivery: drop an
		// event whose full identity — including the daemon `ts` — already sits
		// in the buffer. A leaked/replayed duplicate carries the SAME daemon ts,
		// whereas a legitimately-repeated identical tool call within a turn gets
		// a DIFFERENT ts, so within-turn repeats are preserved.
		if (!this.bufFor(id).push(ev)) return;
		this.streamCbs.emit(id, ev);
	}

	private bufFor(id: string): BoundedEventBuffer {
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

	private setPerms(id: string, list: PermReq[]) {
		this.perms.set(id, list);
		this.changeTick++; // list badge re-derives on changeTick-driven refetch
		this.permCbs.emit(id, list);
	}

	private setAsk(id: string, ask: LiveAsk | null) {
		if (ask === null) this.asks.delete(id);
		else this.asks.set(id, ask);
		this.changeTick++;
		this.askCbs.emit(id, ask);
	}

	private setPlan(id: string, plan: LivePlan | null) {
		if (plan === null) this.plans.delete(id);
		else this.plans.set(id, plan);
		this.changeTick++;
		this.planCbs.emit(id, plan);
	}

	private setSoftLimit(id: string, sl: SoftLimit | null) {
		if (sl === null) this.softLimits.delete(id);
		else this.softLimits.set(id, sl);
		this.changeTick++;
		this.softLimitCbs.emit(id, sl);
	}

	subscribe(id: string) {
		if (!this.subscribed.has(id)) {
			this.subscribed.add(id);
			this.bufFor(id);
			this.send({ type: 'subscribe', session_id: id });
		}
	}

	unsubscribe(id: string) {
		if (this.subscribed.delete(id)) {
			this.send({ type: 'unsubscribe', session_id: id });
		}
	}

	clearStream(id: string) {
		this.bufFor(id).clear();
	}

	/** Snapshot of buffered events for a session (seed for a freshly-opened
	 * view), with any still-pending optimistic replies appended so a sent
	 * message survives a resubscribe until the server echoes it. */
	bufferedEvents(id: string): AgentEvent[] {
		return [...(this.buffer.get(id)?.list() ?? []), ...(this.optimistic.get(id) ?? [])];
	}

	/** Current pending permission count for a session (read in list templates;
	 * the list re-derives on changeTick, which `setPerms` bumps). */
	pendingCount(id: string): number {
		return this.perms.get(id)?.length ?? 0;
	}

	/** Start relaying a session's live terminal. Idempotent — the
	 * server ref-counts watchers and only spins up the daemon PTY stream on the
	 * first watcher. */
	watchPty(id: string) {
		if (!this.ptyWatched.has(id)) {
			this.ptyWatched.add(id);
			this.send({ type: 'watch_terminal', session_id: id, watch: true });
		}
	}

	/** Stop relaying a session's live terminal. */
	unwatchPty(id: string) {
		if (this.ptyWatched.delete(id)) {
			this.send({ type: 'watch_terminal', session_id: id, watch: false });
		}
	}

	/** Register a live PTY-byte listener for a session. Returns an
	 * unsubscribe fn. Bytes are raw terminal output to feed straight into xterm. */
	onPty(id: string, cb: PtyCb): () => void {
		return this.ptyCbs.add(id, cb);
	}

	/** Register a live-event listener for a session. Returns an unsubscribe fn. */
	onStream(id: string, cb: StreamCb): () => void {
		return this.streamCbs.add(id, cb);
	}

	/** Register a live GitHub inbox listener (GH-UI-1). Fires on every
	 * `github_event` broadcast; the inbox uses it to refetch the affected
	 * rows. Returns an unsubscribe fn. Mirrors `onStream`'s callback shape so
	 * the inbox keeps its refresh in component-local `$state`, never reading a
	 * keyed `$state` off this singleton via `$derived`. */
	onGithubEvent(cb: GithubCb): () => void {
		this.githubCbs.add(cb);
		return () => this.githubCbs.delete(cb);
	}

	/** Register a pending-permissions listener for a session. Fires with the
	 * current list immediately and on every change. Returns an unsubscribe fn. */
	onPerms(id: string, cb: PermCb): () => void {
		const off = this.permCbs.add(id, cb);
		cb(this.perms.get(id) ?? []);
		return off;
	}

	/** Register a live AskUserQuestion listener for a session. Fires with the
	 * current pending question (or null) immediately and on every change.
	 * Returns an unsubscribe fn. */
	onAsk(id: string, cb: AskCb): () => void {
		const off = this.askCbs.add(id, cb);
		cb(this.asks.get(id) ?? null);
		return off;
	}

	/** Clear any live pending question for a session (e.g. after the user
	 * answers, before the daemon's resolution event arrives). */
	clearAsk(id: string) {
		if (this.asks.has(id)) this.setAsk(id, null);
	}

	/** Register a live plan-prompt listener for a session. Fires with
	 * the current pending plan (or null) immediately and on every change.
	 * Returns an unsubscribe fn. */
	onPlan(id: string, cb: PlanCb): () => void {
		const off = this.planCbs.add(id, cb);
		cb(this.plans.get(id) ?? null);
		return off;
	}

	/** Clear any live pending plan for a session (e.g. after the user answers,
	 * before the daemon's resolution event arrives). */
	clearPlan(id: string) {
		if (this.plans.has(id)) this.setPlan(id, null);
	}

	/** Register a live soft-limit listener for a session. Fires with
	 * the current block (or null) immediately and on every change. Returns an
	 * unsubscribe fn. Mirrors `onAsk`/`onPlan` so the banner keeps its state in
	 * component-local `$state`, never reading a keyed `$state` off this singleton
	 * via `$derived`. */
	onSoftLimit(id: string, cb: SoftLimitCb): () => void {
		const off = this.softLimitCbs.add(id, cb);
		cb(this.softLimits.get(id) ?? null);
		return off;
	}

	/** Clear any live soft-limit block for a session (e.g. immediately after the
	 * user switches accounts, before the server's `soft_limit_cleared` arrives). */
	clearSoftLimit(id: string) {
		if (this.softLimits.has(id)) this.setSoftLimit(id, null);
	}

	/** Send a typed message. Returns true if the frame went out, false if the
	 * socket wasn't OPEN (caller should keep the draft + surface a notice).
	 * `clientMsgId` opts into a server `message_ack` so the caller can
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

	// ── Tracked send + auto-retry ────────────────────────────────
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
					if (s.phase === 'backoff') retrying.set(s.ts, { attempt: s.attempt, max: MAX_ATTEMPTS });
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
			m?.delete(ts);
		}
		this.notifyDelivery(sid);
	}

	private notifyDelivery(sid: string) {
		if (!this.deliveryCbs.has(sid)) return;
		this.deliveryCbs.emit(sid, this.deliverySnapshot(sid));
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
	 * A timeout is NOT a failure: a cold spawn (kickstarting the
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
			const timer = setTimeout(() => {
				if (this.waiters.delete(commandId)) {
					resolve({ ok: false, timedOut: true, error: 'no spawn confirmation from the daemon' });
				}
			}, timeoutMs);
			this.waiters.set(commandId, (r) => {
				clearTimeout(timer);
				resolve(r);
			});
		});
	}
}

export const ws = new WsClient();
