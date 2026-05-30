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

type Status = 'connecting' | 'open' | 'closed';
type StreamCb = (ev: AgentEvent) => void;
type PermCb = (list: PermReq[]) => void;

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
	/** pending permission prompts, keyed by session id; not reactive */
	private perms = new Map<string, PermReq[]>();
	private streamCbs = new Map<string, Set<StreamCb>>();
	private permCbs = new Map<string, Set<PermCb>>();

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

	private send(frame: Record<string, unknown>) {
		if (this.socket?.readyState === WebSocket.OPEN) {
			this.socket.send(JSON.stringify(frame));
		}
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
		this.buffer.set(id, [...(this.buffer.get(id) ?? []), ev]);
		const set = this.streamCbs.get(id);
		if (set) for (const cb of set) cb(ev);
	}

	private setPerms(id: string, list: PermReq[]) {
		this.perms.set(id, list);
		this.changeTick++; // list badge re-derives on changeTick-driven refetch
		const set = this.permCbs.get(id);
		if (set) for (const cb of set) cb(list);
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

	/** Snapshot of buffered events for a session (seed for a freshly-opened view). */
	bufferedEvents(id: string): AgentEvent[] {
		return [...(this.buffer.get(id) ?? [])];
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

	sendMessage(id: string, content: string) {
		this.send({ type: 'message', session_id: id, content });
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
