import type { AgentEvent } from '@bindings/AgentEvent';
import type { ServerEvent } from '@bindings/ServerEvent';
import type { UserAction } from '@bindings/UserAction';
import type { AccountUsage } from '../queries/types';
import { qk } from '../queries/keys';
import { toasts } from '$lib/toast.svelte';
import { m } from '$lib/paraglide/messages';
import type { QueryClient } from '@tanstack/svelte-query';
import {
	KeyedListeners,
	decodeBase64,
	lastMessageExcerpt,
	scheduledLaunchPatch,
	spawnOutcomeFromEnd,
	userMsgKey,
	type AccountUsageEvent,
	type CommandOutcome,
	type GithubEvent,
	type LifecycleEvent,
	type MachineResourcesEvent,
	type SessionEndedEvent,
	type SessionListPatch
} from './frames';
import { CommandWaiters } from './commands';
import {
	LivePrompts,
	type AskCb,
	type PermCb,
	type PlanCb,
	type SoftLimitCb,
	type ToolBlockCb,
	type UserActionsCb
} from './prompts';
import { SessionStreams, type StreamCb } from './stream';
import { LiveSocket, type Status } from './socket.svelte';
import { DeliveryTracker, type DeliverySnapshot } from './delivery';

export interface CctuiverseChangedEvent {
	session_id: string | null;
	room_id: string | null;
}

type PtyCb = (data: Uint8Array) => void;
type GithubCb = (ev: GithubEvent) => void;

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
	private link = new LiveSocket({
		onOpen: () => {
			// re-subscribe everything after a reconnect
			for (const id of this.subscribed.keys()) this.send({ type: 'subscribe', session_id: id });
			// re-arm live-terminal watches so the daemon PTY stream resumes
			for (const id of this.ptyWatched)
				this.send({ type: 'watch_terminal', session_id: id, watch: true });
			this.delivery.redispatchParked();
		},
		onFrame: (raw) => this.onFrame(raw)
	});

	get status(): Status {
		return this.link.status;
	}

	/** bumped whenever the session set/status changes, so lists can refetch */
	changeTick = $state(0);

	/** Live PTY-view listeners keyed by session id; not reactive. The
	 * bytes are never buffered — a terminal that mounts late relies on the fresh
	 * attach's full-screen repaint, not replay. */
	private ptyCbs = new KeyedListeners<Uint8Array>();
	/** Sessions this client is watching the live terminal of, re-sent on
	 * reconnect so the daemon stream resumes after a drop. */
	private ptyWatched = new Set<string>();
	/** GitHub inbox listeners; not session-keyed — one
	 * broadcast channel the mounted inbox subscribes to. Not reactive. */
	private githubCbs = new Set<GithubCb>();

	private streams = new SessionStreams();
	private prompts = new LivePrompts({
		emitListPatch: (p) => this.emitListPatch(p),
		markListDirty: () => this.markListDirty()
	});
	private commands = new CommandWaiters();
	private delivery = new DeliveryTracker(this);

	/** Subscribers per session, not a flat set: the drawer and any number of
	 *  tiles can hold the same session open, and the last one out is the only
	 *  one allowed to unsubscribe. */
	private subscribed = new Map<string, number>();
	private listDirtyTimer: ReturnType<typeof setTimeout> | null = null;
	private queryClient: QueryClient | null = null;

	/** Lets a `resync` frame refetch the views whose live events were dropped. */
	bindQueryClient(qc: QueryClient) {
		this.queryClient = qc;
	}

	connect() {
		this.link.connect();
	}

	/** Re-establish liveness after a sleep / network change. */
	resumeCheck() {
		this.link.resumeCheck();
	}

	/** Abandon the current socket and dial a new one immediately. */
	forceReconnect() {
		this.link.forceReconnect();
	}

	disconnect() {
		this.link.disconnect();
	}

	/** Write a frame; false if the socket wasn't OPEN and it was dropped. */
	private send(frame: Record<string, unknown>): boolean {
		return this.link.send(frame);
	}

	private onFrame(raw: string) {
		let msg: ServerEvent;
		try {
			msg = JSON.parse(raw);
		} catch {
			return;
		}
		if (this.prompts.handleFrame(msg)) return;
		if ((msg as { type: string }).type === 'cctuiverse_changed') {
			const ev = msg as unknown as CctuiverseChangedEvent;
			for (const cb of this.cctuiverseCbs) cb(ev);
			return;
		}
		switch (msg.type) {
			case 'stream': {
				const { session_id: sid, data } = msg;
				this.commands.settleSpawn(sid, { ok: true });
				this.streams.appendEvent(sid, data);
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
			case 'message_ack': {
				// Resolve the tracked send (delivered / failed → auto-retry).
				this.delivery.resolveAck(msg);
				break;
			}
			case 'command_result': {
				this.commands.resolveCommand(msg.command_id, { ok: msg.ok, error: msg.error ?? undefined });
				break;
			}
			case 'github_event': {
				const ev: GithubEvent = { kind: msg.kind, payload: msg.payload };
				for (const cb of this.githubCbs) cb(ev);
				break;
			}
			case 'session_ended': {
				const { type: _, ...ev } = msg;
				this.commands.settleSpawn(ev.session_id, spawnOutcomeFromEnd(ev.reason, ev.detail ?? null));
				for (const cb of this.sessionEndedCbs) cb(ev);
				this.markListDirty();
				break;
			}
			case 'status':
				this.commands.settleSpawn(msg.session_id, { ok: true });
				this.markListDirty();
				break;
			case 'session_registered':
				this.commands.settleSpawn(msg.session.id, { ok: true });
				this.markListDirty();
				break;
			case 'session_deregistered':
				this.markListDirty();
				break;
			case 'scheduled_launch': {
				const { type: _, ...ev } = msg;
				const patch = scheduledLaunchPatch(ev);
				if (patch) this.emitListPatch(patch);
				else this.markListDirty();
				break;
			}
			// A session moved in or out of a room: the list carries the room as a
			// field, so regrouping is just a refetch.
			case 'room_members':
				this.markListDirty();
				break;
			case 'machine_resources': {
				const { type: _, ...p } = msg;
				for (const cb of this.machineResourcesCbs) cb(p);
				break;
			}
			case 'event': {
				for (const cb of this.eventCbs) cb(msg.event);
				break;
			}
			case 'limit_reset_redeemed':
				toasts.ok(m.auto_limit_reset_redeemed({ account: msg.account_name, outcome: msg.outcome }));
				break;
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

	private cctuiverseCbs = new Set<(ev: CctuiverseChangedEvent) => void>();
	onCctuiverseChanged(cb: (ev: CctuiverseChangedEvent) => void): () => void {
		this.cctuiverseCbs.add(cb);
		return () => this.cctuiverseCbs.delete(cb);
	}

	private accountUsageCbs = new Set<(ev: AccountUsageEvent) => void>();
	onAccountUsage(cb: (ev: AccountUsageEvent) => void): () => void {
		this.accountUsageCbs.add(cb);
		return () => this.accountUsageCbs.delete(cb);
	}

	/** Lifecycle rows as they are recorded; the events page and the session
	 *  Events tab prepend them instead of refetching. */
	private eventCbs = new Set<(ev: LifecycleEvent) => void>();
	onEvent(cb: (ev: LifecycleEvent) => void): () => void {
		this.eventCbs.add(cb);
		return () => this.eventCbs.delete(cb);
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

	subscribe(id: string) {
		const n = this.subscribed.get(id) ?? 0;
		this.subscribed.set(id, n + 1);
		if (n === 0) {
			this.streams.bufFor(id);
			this.send({ type: 'subscribe', session_id: id });
		}
	}

	unsubscribe(id: string) {
		const n = this.subscribed.get(id);
		if (n === undefined) return;
		if (n > 1) {
			this.subscribed.set(id, n - 1);
			return;
		}
		this.subscribed.delete(id);
		this.send({ type: 'unsubscribe', session_id: id });
	}

	/** How many holders a session has — 0 once nobody does. */
	subscriberCount(id: string): number {
		return this.subscribed.get(id) ?? 0;
	}

	private refocusCbs = new Set<() => void>();
	private refocusOff: (() => void) | null = null;

	/**
	 * Fires once per regained-focus / became-visible event, and reconnects the
	 * socket once before fanning out. One listener pair for the whole app: with
	 * a tile per session, per-instance listeners meant N `connect()` calls and N
	 * teardowns for a single focus event.
	 */
	onRefocus(cb: () => void): () => void {
		this.refocusCbs.add(cb);
		if (!this.refocusOff && typeof document !== 'undefined') {
			const fire = () => {
				if (document.visibilityState === 'hidden') return;
				this.connect();
				for (const f of [...this.refocusCbs]) f();
			};
			const onVis = () => {
				if (document.visibilityState === 'visible') fire();
			};
			document.addEventListener('visibilitychange', onVis);
			window.addEventListener('focus', fire);
			this.refocusOff = () => {
				document.removeEventListener('visibilitychange', onVis);
				window.removeEventListener('focus', fire);
			};
		}
		return () => {
			this.refocusCbs.delete(cb);
			if (this.refocusCbs.size === 0) {
				this.refocusOff?.();
				this.refocusOff = null;
			}
		};
	}

	clearStream(id: string) {
		this.streams.bufFor(id).clear();
	}

	bufferedEvents(id: string): AgentEvent[] {
		return this.streams.bufferedEvents(id);
	}

	recordOptimistic(id: string, ev: AgentEvent) {
		this.streams.recordOptimistic(id, ev);
	}

	dropOptimistic(id: string, ts: number) {
		this.streams.dropOptimistic(id, ts);
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

	onStream(id: string, cb: StreamCb): () => void {
		return this.streams.onStream(id, cb);
	}

	/** Register a live GitHub inbox listener. Fires on every
	 * `github_event` broadcast; the inbox uses it to refetch the affected
	 * rows. Returns an unsubscribe fn. Mirrors `onStream`'s callback shape so
	 * the inbox keeps its refresh in component-local `$state`, never reading a
	 * keyed `$state` off this singleton via `$derived`. */
	onGithubEvent(cb: GithubCb): () => void {
		this.githubCbs.add(cb);
		return () => this.githubCbs.delete(cb);
	}

	pendingCount(id: string): number {
		return this.prompts.pendingCount(id);
	}

	onPerms(id: string, cb: PermCb): () => void {
		return this.prompts.onPerms(id, cb);
	}

	onAsk(id: string, cb: AskCb): () => void {
		return this.prompts.onAsk(id, cb);
	}

	clearAsk(id: string) {
		this.prompts.clearAsk(id);
	}

	onPlan(id: string, cb: PlanCb): () => void {
		return this.prompts.onPlan(id, cb);
	}

	clearPlan(id: string) {
		this.prompts.clearPlan(id);
	}

	onSoftLimit(id: string, cb: SoftLimitCb): () => void {
		return this.prompts.onSoftLimit(id, cb);
	}

	onToolBlock(id: string, cb: ToolBlockCb): () => void {
		return this.prompts.onToolBlock(id, cb);
	}

	onUserActions(id: string, cb: UserActionsCb): () => void {
		return this.prompts.onUserActions(id, cb);
	}

	/** Seed the list from an HTTP read (the WS only pushes changes). */
	setUserActions(id: string, list: UserAction[]) {
		this.prompts.setUserActions(id, list);
	}

	dismissToolBlock(id: string) {
		this.prompts.dismissToolBlock(id);
	}

	clearSoftLimit(id: string) {
		this.prompts.clearSoftLimit(id);
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

	respondPermission(sessionId: string, requestId: string, allow: boolean, optionId?: string) {
		this.send({
			type: 'permission_response',
			session_id: sessionId,
			request_id: requestId,
			behavior: allow ? 'allow' : 'deny',
			...(optionId ? { option_id: optionId } : {})
		});
		this.prompts.removePerm(sessionId, requestId);
	}

	/** Resolve when the server reports a result for `commandId`; see
	 * `CommandWaiters.awaitCommand`. */
	awaitCommand(commandId: string, timeoutMs?: number): Promise<CommandOutcome> {
		return this.commands.awaitCommand(commandId, timeoutMs);
	}

	/** Resolve a spawn on its first signal; see `CommandWaiters.awaitSpawn`. */
	awaitSpawn(
		commandId: string,
		sessionId: string | null | undefined,
		opts: Parameters<CommandWaiters['awaitSpawn']>[2] = {}
	): Promise<CommandOutcome> {
		return this.commands.awaitSpawn(commandId, sessionId, opts);
	}
}

export const ws = new WsClient();
