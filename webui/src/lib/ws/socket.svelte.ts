import { browser } from '$app/environment';
import { wsBase } from '../config';
import { auth } from '../auth.svelte';
import { net } from '../netstats.svelte';

export type Status = 'connecting' | 'open' | 'closed';

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

export interface SocketHooks {
	/** The socket (re)opened: restore server-side subscriptions. */
	onOpen(): void;
	onFrame(raw: string): void;
}

/** The TUI websocket connection: dials, keeps liveness with a watchdog, and
 * redials with backoff while wanted. */
export class LiveSocket {
	status = $state<Status>('closed');

	private socket: WebSocket | null = null;
	private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
	private want = false;
	private watchdogTimer: ReturnType<typeof setTimeout> | null = null;
	private lastFrameAt = 0;
	private lifecycleBound = false;

	constructor(private hooks: SocketHooks) {}

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
		// automatically, so the token never rides the query string.
		const url = `${wsBase()}/ws`;
		const sock = new WebSocket(url);
		this.socket = sock;

		sock.onopen = () => {
			if (this.socket !== sock) return;
			this.status = 'open';
			this.lastFrameAt = Date.now();
			this.armWatchdog();
			this.hooks.onOpen();
		};
		sock.onmessage = (ev) => {
			if (this.socket !== sock) return;
			this.lastFrameAt = Date.now();
			this.armWatchdog();
			if (typeof ev.data === 'string') net.recordWs(ev.data.length);
			this.hooks.onFrame(ev.data);
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
	send(frame: Record<string, unknown>): boolean {
		if (this.socket?.readyState === WebSocket.OPEN) {
			this.socket.send(JSON.stringify(frame));
			return true;
		}
		return false;
	}
}
