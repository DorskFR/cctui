import type { Notification as Notice } from '@bindings/Notification';
import type { NotificationList } from '@bindings/NotificationList';
import type { NotificationTransition } from '@bindings/NotificationTransition';
import type { NotificationReadAll } from '@bindings/NotificationReadAll';
import type { NotificationActRequest } from '@bindings/NotificationActRequest';
import type { NotificationAction, NotificationData, NotificationTone } from '@dorsk/tsumikit';
import { ApiError } from './api';
import { endpoints } from './queries/endpoints';
import { m } from '$lib/paraglide/messages';

const PAGE = 50;
const UNREAD = new Set(['pending', 'delivered']);
const TERMINAL = new Set(['resolved', 'expired']);

export const isUnread = (n: Notice) => UNREAD.has(n.state);
export const isTerminal = (n: Notice) => TERMINAL.has(n.state);

export interface NotificationApi {
	list(q: { state: 'open'; limit: number; before?: string }): Promise<NotificationList>;
	read(id: string): Promise<NotificationTransition>;
	act(id: string, body: NotificationActRequest): Promise<NotificationTransition>;
	dismiss(id: string): Promise<NotificationTransition>;
	readAll(): Promise<NotificationReadAll>;
}

const http: NotificationApi = {
	list: (q) => endpoints.notifications(q),
	read: (id) => endpoints.readNotification(id),
	act: (id, body) => endpoints.actNotification(id, body),
	dismiss: (id) => endpoints.dismissNotification(id),
	readAll: () => endpoints.readAllNotifications()
};

export interface NotificationSource {
	onNotification(cb: (n: Notice) => void): () => void;
	onNotificationsUnread(cb: (count: number) => void): () => void;
}

export type ActOutcome =
	| { kind: 'applied'; notification: Notice }
	| { kind: 'elsewhere'; notification: Notice }
	| { kind: 'failed'; error: unknown };

export type DismissOutcome = 'dismissed' | 'blocking' | 'failed';

type RawAction = { id?: unknown; label?: unknown; style?: unknown };

export function noticeActions(n: Notice): NotificationAction[] {
	if (!Array.isArray(n.actions)) return [];
	return (n.actions as RawAction[]).flatMap((a) => {
		if (typeof a?.id !== 'string') return [];
		const style = a.style === 'danger' || a.style === 'primary' ? a.style : undefined;
		return [
			{
				id: a.id,
				label: typeof a.label === 'string' ? a.label : a.id,
				variant: style,
				requiresConfirm: style === 'danger' || undefined
			}
		];
	});
}

function tone(severity: string): NotificationTone {
	if (severity === 'critical') return 'danger';
	if (severity === 'high') return 'warn';
	return 'info';
}

export function stateLabel(n: Notice): string | undefined {
	if (n.state !== 'acted' && !isTerminal(n)) return undefined;
	const decision = n.decision ?? n.state;
	return n.acted_channel
		? m.notif_state_on({ decision, channel: n.acted_channel })
		: m.notif_state({ decision });
}

export function toItem(n: Notice): NotificationData {
	const state = stateLabel(n);
	return {
		id: n.id,
		title: n.title,
		text: n.body ?? undefined,
		time: n.created_at,
		tone: tone(n.severity),
		unread: isUnread(n),
		state,
		actions: state ? [] : noticeActions(n)
	};
}

const newestFirst = (a: Notice, b: Notice) => b.created_at.localeCompare(a.created_at);

export class NotificationStore {
	items = $state<Notice[]>([]);
	unread = $state(0);
	hasMore = $state(false);
	loading = $state(false);
	#api: NotificationApi;
	#reading = new Set<string>();

	constructor(api: NotificationApi = http) {
		this.#api = api;
	}

	upsert(n: Notice) {
		const rest = this.items.filter((x) => x.id !== n.id);
		this.items = isTerminal(n) ? rest : [...rest, n].sort(newestFirst);
	}

	setUnread(count: number) {
		this.unread = Math.max(0, count);
	}

	connect(source: NotificationSource): () => void {
		const offs = [
			source.onNotification((n) => this.upsert(n)),
			source.onNotificationsUnread((c) => this.setUnread(c))
		];
		return () => {
			for (const off of offs) off();
		};
	}

	async load() {
		this.loading = true;
		try {
			const page = await this.#api.list({ state: 'open', limit: PAGE });
			this.items = page.notifications.filter((n) => !isTerminal(n)).sort(newestFirst);
			this.hasMore = page.has_more;
			this.setUnread(page.unread);
		} finally {
			this.loading = false;
		}
	}

	async loadMore() {
		const last = this.items.at(-1);
		if (!last || this.loading) return;
		this.loading = true;
		try {
			const page = await this.#api.list({ state: 'open', limit: PAGE, before: last.created_at });
			for (const n of page.notifications) if (!this.items.some((x) => x.id === n.id)) this.upsert(n);
			this.hasMore = page.has_more;
			this.setUnread(page.unread);
		} finally {
			this.loading = false;
		}
	}

	/** Reading cancels a pending push escalation, so it must mean "seen". */
	async markRead(id: string) {
		const n = this.items.find((x) => x.id === id);
		if (!n || !isUnread(n) || this.#reading.has(id)) return;
		this.#reading.add(id);
		try {
			this.upsert((await this.#api.read(id)).notification);
		} catch {
			/* the next list load carries the real state */
		} finally {
			this.#reading.delete(id);
		}
	}

	async act(id: string, action: string): Promise<ActOutcome> {
		try {
			const t = await this.#api.act(id, { action, note: null, channel: 'web' });
			this.upsert(t.notification);
			return { kind: t.applied ? 'applied' : 'elsewhere', notification: t.notification };
		} catch (error) {
			return { kind: 'failed', error };
		}
	}

	async dismiss(id: string): Promise<DismissOutcome> {
		try {
			this.upsert((await this.#api.dismiss(id)).notification);
			return 'dismissed';
		} catch (e) {
			return e instanceof ApiError && e.status === 409 ? 'blocking' : 'failed';
		}
	}

	async readAll() {
		const done = await this.#api.readAll();
		this.setUnread(done.unread);
		await this.load();
	}
}

export const notifications = new NotificationStore();
