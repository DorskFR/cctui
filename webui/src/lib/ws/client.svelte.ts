import { browser } from '$app/environment';
import { wsBase } from '../config';
import { auth } from '../auth.svelte';
import { net } from '../netstats.svelte';
import type { AgentEvent } from '@bindings/AgentEvent';
import type { ServerEvent } from '@bindings/ServerEvent';
import type { AccountUsage } from '../queries/types';
import { qk } from '../queries/keys';
import type { QueryClient } from '@tanstack/svelte-query';
import {
	BoundedEventBuffer,
	KeyedListeners,
	decodeBase64,
	lastMessageExcerpt,
	spawnOutcomeFromEnd,
	turnIdOf,
	userMsgKey,
	type AccountUsageEvent,
	type CommandOutcome,
	type GithubEvent,
	type LiveAsk,
	type LivePlan,
	type MachineResourcesEvent,
	type PermReq,
	type SessionEndedEvent,
	type SessionListPatch,
	type SoftLimitBlock,
	type SpawnProbeHit,
	type ToolBlock
} from './frames';
import { DeliveryTracker, type DeliverySnapshot } from './delivery';

/** Daemon handshake budget (45 s) plus dispatch and relay slack. */
export const SPAWN_ACK_TIMEOUT_MS = 75_000;
/** How often `awaitSpawn` re-derives the outcome from the session list. */
export const SPAWN_PROBE_INTERVAL_MS = 5_000;


type Status = 'connecting' | 'open' | 'closed';
type StreamCb = (ev: AgentEvent) => void;
type PtyCb = (data: Uint8Array) => void;
type PermCb = (list: PermReq[]) => void;
type GithubCb = (ev: GithubEvent) => void;
/** Live AskUserQuestion for a session, or null when none is pending. */
type AskCb = (ask: LiveAsk | null) => void;
/** Live plan prompt for a session, or null when none is pending. */
type PlanCb = (plan: LivePlan | null) => void;
/** Live soft-limit block for a session, or null when none is active. */
type SoftLimitCb = (sl: SoftLimitBlock | null) => void;
type ToolBlockCb = (b: ToolBlock | null) => void;




/**
 * Silence after which the socket is presumed half-open. A browser cannot
 * observe the server's `Ping`/`Pong`, so liveness is inferred from frames; the
 * server's `heartbeat` arrives every ~20 s on every socket, independently of
 * whether any daemon is online. A half-open connection never fires `onclose`,
 * so nothing else detects it.
 */
export const WATCHDOG_MS = 60_000;
/** Silence after which a tab-visible / network-online socket is presumed dead
 *  rather than merely idle. */
export const RESUME_STALE_MS = 30_000;


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
export class WsClient {
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
	private softLimits = new Map<string, SoftLimitBlock>();
	private toolBlocks = new Map<string, ToolBlock>();
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
	private softLimitCbs = new KeyedListeners<SoftLimitBlock | null>();
	private toolBlockCbs = new KeyedListeners<ToolBlock | null>();
	/** GitHub inbox listeners (GH-CONN-5 / GH-UI-1); not session-keyed — one
	 * broadcast channel the mounted inbox subscribes to. Not reactive. */
	private githubCbs = new Set<GithubCb>();

	private delivery = new DeliveryTracker(this);

	private socket: WebSocket | null = null;
	private subscribed = new Set<string>();
	private waiters = new Map<string, (r: CommandOutcome) => void>();
	/** Spawns waiting on their pre-minted session id to show up on this socket. */
	private spawnWaiters = new Map<string, (r: CommandOutcome) => void>();
	private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
	private listDirtyTimer: ReturnType<typeof setTimeout> | null = null;
	private want = false;
	private watchdogTimer: ReturnType<typeof setTimeout> | null = null;
	private lastFrameAt = 0;
	private lifecycleBound = false;
	private queryClient: QueryClient | null = null;

	/** Lets a `resync` frame refetch the views whose live events were dropped. */
	bindQueryClient(qc: QueryClient) {
		this.queryClient = qc;
	}

	connect() {
		if (!browser || !auth.isAuthed) return;
		this.want = true;
		this.bindLifecycle();
		this.open();
	}

	private bindLifecycle() {
		if (this.lifecycleBound || typeof document === 'undefined') return;
		this.lifecycleBound = true;
		document.addEventListener('visibilitychange', () => {
			if (document.visibilityState === 'visible') this.resumeCheck();
		});
		window.addEventListener('online', () => this.resumeCheck());
	}

	/** Re-establish liveness after a sleep / network change: dial if the socket
	 * is gone, force a fresh one if it reads `OPEN` but has gone quiet. */
	resumeCheck() {
		if (!this.want) return;
		if (!this.socket || this.socket.readyState !== WebSocket.OPEN) {
			this.open();
			return;
		}
		if (Date.now() - this.lastFrameAt > RESUME_STALE_MS) this.forceReconnect();
	}

	private armWatchdog() {
		this.clearWatchdog();
		this.watchdogTimer = setTimeout(() => {
			this.watchdogTimer = null;
			this.forceReconnect();
		}, WATCHDOG_MS);
	}

	private clearWatchdog() {
		if (this.watchdogTimer) {
			clearTimeout(this.watchdogTimer);
			this.watchdogTimer = null;
		}
	}

	/** Abandon the current socket and dial a new one immediately. The old
	 * socket's handlers are inert afterwards: they all bail unless they are
	 * still the live socket. */
	forceReconnect() {
		this.clearWatchdog();
		const sock = this.socket;
		this.socket = null;
		this.status = 'closed';
		sock?.close();
		if (this.want) this.open();
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
			if (this.socket !== sock) return;
			this.status = 'open';
			this.lastFrameAt = Date.now();
			this.armWatchdog();
			// re-subscribe everything after a reconnect
			for (const id of this.subscribed) this.send({ type: 'subscribe', session_id: id });
			// re-arm live-terminal watches so the daemon PTY stream resumes
			for (const id of this.ptyWatched)
				this.send({ type: 'watch_terminal', session_id: id, watch: true });
			this.delivery.redispatchParked();
		};
		sock.onmessage = (ev) => {
			if (this.socket !== sock) return;
			this.lastFrameAt = Date.now();
			this.armWatchdog();
			if (typeof ev.data === 'string') net.recordWs(ev.data.length);
			this.onFrame(ev.data);
		};
		sock.onclose = () => {
			if (this.socket !== sock) return;
			this.clearWatchdog();
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
		this.clearWatchdog();
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
		let msg: ServerEvent;
		try {
			msg = JSON.parse(raw);
		} catch {
			return;
		}
		switch (msg.type) {
			case 'stream': {
				const { session_id: sid, data } = msg;
				this.settleSpawn(sid, { ok: true });
				this.appendEvent(sid, data);
				// The list's last-message column tracks USER messages only
				// (server: event_type='message'); assistant text must not patch it.
				if (userMsgKey(data) !== null && (data.type === 'text' || data.type === 'reply')) {
					this.emitListPatch({
						session_id: sid,
						last_message_text: lastMessageExcerpt(data.content),
						last_message_at: new Date(data.ts).toISOString()
					});
				}
				break;
			}
			case 'pty_chunk': {
				if (this.ptyCbs.has(msg.session_id)) this.ptyCbs.emit(msg.session_id, decodeBase64(msg.data));
				break;
			}
			case 'permission_request': {
				const { type: _, ...p } = msg;
				const list = this.perms.get(p.session_id) ?? [];
				if (!list.some((x) => x.request_id === p.request_id)) {
					this.setPerms(p.session_id, [...list, p]);
				}
				break;
			}
			case 'permission_resolved': {
				const sid = msg.session_id;
				this.setPerms(
					sid,
					(this.perms.get(sid) ?? []).filter((x) => x.request_id !== msg.request_id)
				);
				break;
			}
			case 'ask_question': {
				this.setAsk(msg.session_id, {
					question: msg.question,
					questions: msg.questions ?? null,
					preamble: msg.preamble ?? null
				});
				break;
			}
			case 'ask_resolved':
				this.setAsk(msg.session_id, null);
				break;
			case 'plan_request': {
				this.setPlan(msg.session_id, { plan: msg.plan, preamble: msg.preamble ?? null });
				break;
			}
			case 'plan_resolved':
				this.setPlan(msg.session_id, null);
				break;
			case 'soft_limit_reached': {
				const { type: _, session_id, ...sl } = msg;
				this.setSoftLimit(session_id, sl);
				break;
			}
			case 'tool_call_blocked': {
				this.setToolBlock(msg.session_id, { tool_name: msg.tool_name, rule: msg.rule });
				break;
			}
			case 'soft_limit_cleared':
				this.setSoftLimit(msg.session_id, null);
				break;
			case 'message_ack': {
				// Resolve the tracked send (delivered / failed → auto-retry).
				this.delivery.resolveAck(msg);
				break;
			}
			case 'command_result': {
				const w = this.waiters.get(msg.command_id);
				if (w) {
					w({ ok: msg.ok, error: msg.error ?? undefined });
					this.waiters.delete(msg.command_id);
				}
				break;
			}
			case 'github_event': {
				const ev: GithubEvent = { kind: msg.kind, payload: msg.payload };
				for (const cb of this.githubCbs) cb(ev);
				break;
			}
			case 'session_ended': {
				const { type: _, ...ev } = msg;
				this.settleSpawn(ev.session_id, spawnOutcomeFromEnd(ev.reason, ev.detail ?? null));
				for (const cb of this.sessionEndedCbs) cb(ev);
				this.markListDirty();
				break;
			}
			case 'status':
				this.settleSpawn(msg.session_id, { ok: true });
				this.markListDirty();
				break;
			case 'session_registered':
				this.settleSpawn(msg.session.id, { ok: true });
				this.markListDirty();
				break;
			case 'session_deregistered':
				this.markListDirty();
				break;
			case 'machine_resources': {
				const { type: _, ...p } = msg;
				for (const cb of this.machineResourcesCbs) cb(p);
				break;
			}
			case 'account_usage': {
				const p: AccountUsageEvent = {
					account_id: msg.account_id,
					usage: msg.usage as unknown as AccountUsage
				};
				for (const cb of this.accountUsageCbs) cb(p);
				break;
			}
			case 'resync': {
				const sid = msg.session_id;
				void this.queryClient?.invalidateQueries({
					queryKey: sid ? qk.conversation(sid) : qk.conversationAll
				});
				if (!sid) {
					void this.queryClient?.invalidateQueries({ queryKey: qk.sessionsAll });
					this.markListDirty();
				}
				break;
			}
		}
	}

	/** Live host resource snapshots, one per daemon heartbeat, for the header
	 *  gauge to patch its query cache without a refetch. */
	private machineResourcesCbs = new Set<(ev: MachineResourcesEvent) => void>();
	onMachineResources(cb: (ev: MachineResourcesEvent) => void): () => void {
		this.machineResourcesCbs.add(cb);
		return () => this.machineResourcesCbs.delete(cb);
	}

	private accountUsageCbs = new Set<(ev: AccountUsageEvent) => void>();
	onAccountUsage(cb: (ev: AccountUsageEvent) => void): () => void {
		this.accountUsageCbs.add(cb);
		return () => this.accountUsageCbs.delete(cb);
	}

	private sessionEndedCbs = new Set<(ev: SessionEndedEvent) => void>();
	onSessionEnded(cb: (ev: SessionEndedEvent) => void): () => void {
		this.sessionEndedCbs.add(cb);
		return () => this.sessionEndedCbs.delete(cb);
	}

	/** Debounced list-refresh trigger: bumps changeTick at most ~once/2s. */
	private markListDirty() {
		if (this.listDirtyTimer) return;
		this.listDirtyTimer = setTimeout(() => {
			this.listDirtyTimer = null;
			this.changeTick++;
		}, 2000);
	}

	// Per-session in-place patches; full refetches are reserved for structural
	// changes and the 15s poll reconciles the rest.
	private listPatchCbs = new Set<(p: SessionListPatch) => void>();
	onListPatch(cb: (p: SessionListPatch) => void): () => void {
		this.listPatchCbs.add(cb);
		return () => this.listPatchCbs.delete(cb);
	}
	private emitListPatch(p: SessionListPatch) {
		for (const cb of this.listPatchCbs) cb(p);
	}

	private appendEvent(id: string, ev: AgentEvent) {
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
				// turn id on the way back is reconciled exactly as before.
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

	// Gaining attention patches the list item in place (the client knows the
	// session just became blocked); losing it needs the server-derived bucket,
	// so that path falls back to the debounced refetch.
	private setPerms(id: string, list: PermReq[]) {
		this.perms.set(id, list);
		if (list.length > 0) {
			this.emitListPatch({ session_id: id, attention: 'needs_input', bucket: 'blocked' });
		} else {
			this.markListDirty();
		}
		this.permCbs.emit(id, list);
	}

	private setAsk(id: string, ask: LiveAsk | null) {
		if (ask === null) this.asks.delete(id);
		else this.asks.set(id, ask);
		if (ask) this.emitListPatch({ session_id: id, attention: 'needs_input', bucket: 'blocked' });
		else this.markListDirty();
		this.askCbs.emit(id, ask);
	}

	private setPlan(id: string, plan: LivePlan | null) {
		if (plan === null) this.plans.delete(id);
		else this.plans.set(id, plan);
		if (plan) this.emitListPatch({ session_id: id, attention: 'needs_input', bucket: 'blocked' });
		else this.markListDirty();
		this.planCbs.emit(id, plan);
	}

	private setSoftLimit(id: string, sl: SoftLimitBlock | null) {
		if (sl === null) this.softLimits.delete(id);
		else this.softLimits.set(id, sl);
		this.markListDirty();
		this.softLimitCbs.emit(id, sl);
	}

	private setToolBlock(id: string, b: ToolBlock | null) {
		if (b === null) this.toolBlocks.delete(id);
		else this.toolBlocks.set(id, b);
		this.toolBlockCbs.emit(id, b);
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

	/** Latest tool-call block for a session, until dismissed. Fires with the
	 * current one immediately. */
	onToolBlock(id: string, cb: ToolBlockCb): () => void {
		const off = this.toolBlockCbs.add(id, cb);
		cb(this.toolBlocks.get(id) ?? null);
		return off;
	}

	dismissToolBlock(id: string) {
		if (this.toolBlocks.has(id)) this.setToolBlock(id, null);
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
	sendMessage(
		id: string,
		content: string,
		clientMsgId?: string,
		askPicks?: number[][],
		turnId?: string
	): boolean {
		return this.send({
			type: 'message',
			session_id: id,
			content,
			...(clientMsgId ? { client_msg_id: clientMsgId } : {}),
			...(askPicks ? { ask_picks: askPicks } : {}),
			...(turnId ? { turn_id: turnId } : {})
		});
	}


	/** Begin tracking + dispatching a send; see `DeliveryTracker.trackedSend`. */
	trackedSend(
		sid: string,
		text: string,
		ts: number,
		askPicks?: number[][],
		turnId?: string
	): boolean {
		return this.delivery.trackedSend(sid, text, ts, askPicks, turnId);
	}

	retryNow(sid: string, ts: number) {
		this.delivery.retryNow(sid, ts);
	}

	cancelSend(sid: string, ts: number) {
		this.delivery.cancelSend(sid, ts);
	}

	clearDelivery(sid: string) {
		this.delivery.clearDelivery(sid);
	}

	deliverySnapshot(sid: string): DeliverySnapshot {
		return this.delivery.deliverySnapshot(sid);
	}

	onDelivery(sid: string, cb: (snap: DeliverySnapshot) => void): () => void {
		return this.delivery.onDelivery(sid, cb);
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
	 * The daemon fails a codex handshake at 45 s, so the wait outlasts that
	 * plus dispatch latency: a real failure must arrive here, not after the
	 * caller gave up. A timeout is still NOT a failure: a cold spawn
	 * (kickstarting the agent daemon, staging uploads) can outlive any
	 * client-side wait, and the session still lands. `timedOut` lets the caller
	 * phrase it as "unconfirmed, check the list" instead of an error inviting a
	 * retry — re-submitting dispatches a brand-new spawn and a duplicate agent.
	 */
	awaitCommand(commandId: string, timeoutMs = SPAWN_ACK_TIMEOUT_MS): Promise<CommandOutcome> {
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

	private settleSpawn(sessionId: string | undefined, r: CommandOutcome) {
		if (!sessionId) return;
		const w = this.spawnWaiters.get(sessionId);
		if (!w) return;
		this.spawnWaiters.delete(sessionId);
		w(r);
	}

	/** Resolve a spawn on whichever lands first: the daemon's `command_result`
	 * for `commandId`, any event for the pre-minted `sessionId` on this
	 * socket, or `probe` finding the session's row. The ack is the only one of
	 * those the server never replays, so the other two cover a lost or late
	 * frame; the timeout is the last resort and still not a failure (see
	 * `awaitCommand`). */
	awaitSpawn(
		commandId: string,
		sessionId: string | null | undefined,
		opts: {
			probe?: () => Promise<SpawnProbeHit | null>;
			probeIntervalMs?: number;
			timeoutMs?: number;
		} = {}
	): Promise<CommandOutcome> {
		if (!sessionId) return this.awaitCommand(commandId, opts.timeoutMs);
		const interval = opts.probeIntervalMs ?? SPAWN_PROBE_INTERVAL_MS;
		return new Promise((resolve) => {
			let settled = false;
			let probeTimer: ReturnType<typeof setTimeout> | null = null;
			const finish = (r: CommandOutcome) => {
				if (settled) return;
				settled = true;
				this.waiters.delete(commandId);
				this.spawnWaiters.delete(sessionId);
				if (probeTimer) clearTimeout(probeTimer);
				resolve(r);
			};
			void this.awaitCommand(commandId, opts.timeoutMs).then(finish);
			this.spawnWaiters.set(sessionId, finish);
			const { probe } = opts;
			if (!probe) return;
			const tick = async () => {
				if (settled) return;
				let hit: SpawnProbeHit | null = null;
				try {
					hit = await probe();
				} catch {
					hit = null;
				}
				if (settled) return;
				if (hit) {
					finish(spawnOutcomeFromEnd(hit.end_reason ?? null, hit.end_detail ?? null));
					return;
				}
				probeTimer = setTimeout(() => void tick(), interval);
			};
			probeTimer = setTimeout(() => void tick(), interval);
		});
	}
}

export const ws = new WsClient();
