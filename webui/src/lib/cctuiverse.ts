import type { CctuiverseLinkView } from '@bindings/CctuiverseLinkView';
import { api } from './api';

export type LinkView = CctuiverseLinkView;
export type LinkSettings = CctuiverseLinkView['settings'];

export interface LinkMessage {
	id: number;
	message_id: string;
	direction: 'in' | 'out';
	kind: 'direct' | 'room_post' | 'close';
	text: string;
	status: string;
	created_at: string;
}

export type InviteTarget = { session: string } | { room: string };

const enc = encodeURIComponent;
const targetPath = (t: InviteTarget) =>
	'session' in t ? `/sessions/${enc(t.session)}` : `/rooms/${enc(t.room)}`;

export const getConfig = () => api.get<{ enabled: boolean }>('/cctuiverse/config');

export const createInvite = (target: InviteTarget, label: string) =>
	api.post<{ link: LinkView; invite: string }>(`${targetPath(target)}/cctuiverse/invites`, {
		label
	});

export const joinInvite = (invite: string, sessionId: string, label: string) =>
	api
		.post<{ link: LinkView }>('/cctuiverse/join', { invite, session_id: sessionId, label })
		.then((r) => r.link);

export const listLinks = (target: InviteTarget) =>
	api
		.get<{ links: LinkView[] }>(`${targetPath(target)}/cctuiverse/links`)
		.then((r) => r.links);

export const updateLink = (id: string, patch: Partial<LinkSettings>) =>
	api.patch<{ link: LinkView }>(`/cctuiverse/links/${enc(id)}`, patch).then((r) => r.link);

export const closeLink = (id: string) =>
	api.post<{ link: LinkView }>(`/cctuiverse/links/${enc(id)}/close`).then((r) => r.link);

export const listLinkMessages = (id: string, status: 'held' | 'review') =>
	api
		.get<{ messages: LinkMessage[] }>(`/cctuiverse/links/${enc(id)}/messages`, { status })
		.then((r) => r.messages);

export type MessageAction = 'release' | 'drop' | 'approve';

export const actOnMessage = (linkId: string, msgId: number, action: MessageAction) =>
	api.post<unknown>(`/cctuiverse/links/${enc(linkId)}/messages/${msgId}/${action}`);

export const JOIN_PROMPT =
	"You are joining a cctuiverse discussion. Wait for the other agent's first message, or introduce yourself with CctuiSend once you are linked.";

const INVITE_RE = /https?:\/\/\S+\/cctuiverse\/join#v1\.\S+/;

export function findInvite(text: string): string | null {
	return INVITE_RE.exec(text)?.[0] ?? null;
}

export function stripInvite(text: string, invite: string, fallback: string): string {
	const rest = text.replace(invite, '').trim();
	return rest || fallback;
}

export const openLinks = (links: LinkView[]): LinkView[] =>
	links.filter((l) => l.state !== 'closed');

export type ExpiryChoice = '24h' | '7d' | 'never';

const HOUR = 3_600_000;

export function expiryFor(choice: ExpiryChoice, now: number): string | null {
	if (choice === 'never') return null;
	return new Date(now + (choice === '24h' ? 24 : 24 * 7) * HOUR).toISOString();
}

export function remainingMs(iso: string | null, now: number): number | null {
	if (!iso) return null;
	return Math.max(0, Date.parse(iso) - now);
}

export function formatCountdown(ms: number): string {
	const s = Math.floor(ms / 1000);
	const h = Math.floor(s / 3600);
	const min = Math.floor((s % 3600) / 60);
	const pad = (n: number) => String(n).padStart(2, '0');
	return h > 0 ? `${h}h ${pad(min)}m` : `${min}m ${pad(s % 60)}s`;
}
