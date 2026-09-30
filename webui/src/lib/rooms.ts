// Room client: the REST calls plus the pure helpers the Room panel and the
// session-card badge share. Kept side-effect free apart from `api.*` so the
// grouping/labelling logic is testable without a component.
import { api } from './api';

export type RoomMemberRole = 'member' | 'observer';

export interface RoomMember {
	session_id: string;
	name: string | null;
	adapter: string | null;
	machine: string | null;
	state: 'live' | 'ended' | 'archived';
	role: RoomMemberRole;
	last_delivered_seq: number;
}

export interface Room {
	id: string;
	name: string;
	archived: boolean;
	members: RoomMember[];
}

export interface RoomMessage {
	seq: number;
	/** null is the human. */
	sender_session_id: string | null;
	sender_label: string;
	body: string;
	created_at: string;
}

/** A room as the card badge needs it: no members, plus this session's role. */
export interface SessionRoom {
	id: string;
	name: string;
	role: RoomMemberRole;
}

export const listRooms = () => api.get<{ rooms: Room[] }>('/rooms').then((r) => r.rooms);

export const getRoom = (id: string) => api.get<Room>(`/rooms/${id}`);

export const createRoom = (name: string, sessionIds: string[] = []) =>
	api.post<Room>('/rooms', { name, session_ids: sessionIds });

export const renameRoom = (id: string, name: string) => api.patch<Room>(`/rooms/${id}`, { name });

export const setRoomArchived = (id: string, archived: boolean) =>
	api.patch<Room>(`/rooms/${id}`, { archived });

export const deleteRoom = (id: string) => api.del<void>(`/rooms/${id}`);

export const addRoomMember = (id: string, sessionId: string, role: RoomMemberRole = 'member') =>
	api.post<RoomMember>(`/rooms/${id}/members`, { session_id: sessionId, role });

export const removeRoomMember = (id: string, sessionId: string) =>
	api.del<void>(`/rooms/${id}/members/${sessionId}`);

export const roomMessages = (id: string, after?: number) =>
	api
		.get<{ messages: RoomMessage[] }>(`/rooms/${id}/messages`, { after })
		.then((r) => r.messages);

export const postToRoom = (id: string, message: string) =>
	api.post<RoomMessage>(`/rooms/${id}/messages`, { message });

export const sessionRooms = (sessionId: string) =>
	api.get<{ rooms: SessionRoom[] }>(`/sessions/${sessionId}/rooms`).then((r) => r.rooms);

// --- peer shares (the writer rooms brings with it) ---

export interface PeerShare {
	session_id: string;
	name: string | null;
}

export const listPeerShares = (sessionId: string) =>
	api.get<{ shares: PeerShare[] }>(`/sessions/${sessionId}/peer-shares`).then((r) => r.shares);

export const sharePeer = (sessionId: string, peerSessionId: string) =>
	api.post<unknown>(`/sessions/${sessionId}/peer-shares`, { peer_session_id: peerSessionId });

export const unsharePeer = (sessionId: string, peerSessionId: string) =>
	api.del<void>(`/sessions/${sessionId}/peer-shares/${peerSessionId}`);

// --- pure helpers ---

/** Mirrors the server cap so the composer can refuse before the round-trip. */
export const MAX_POST_BYTES = 16 * 1024;

/** Why a composer post cannot be sent, or null when it can. */
export function postProblem(
	room: Pick<Room, 'archived'>,
	body: string
): 'empty' | 'too-large' | 'archived' | null {
	if (room.archived) return 'archived';
	const trimmed = body.trim();
	if (!trimmed) return 'empty';
	if (new TextEncoder().encode(trimmed).length > MAX_POST_BYTES) return 'too-large';
	return null;
}

/** A member as one line: `name (adapter on machine)`, falling back to the id. */
export function memberLabel(m: RoomMember): string {
	const name = m.name?.trim() || m.session_id;
	return `${name} (${m.adapter ?? 'unknown'} on ${m.machine ?? 'unknown machine'})`;
}

/** A member that can no longer receive posts is shown greyed out. */
export const isDormant = (m: RoomMember) => m.state !== 'live';

/**
 * Merge freshly fetched messages into what the panel already holds, keyed by
 * `seq`. The WS event and the `after=` poll both deliver the same post, so the
 * panel would double it without this.
 */
export function mergeMessages(have: RoomMessage[], incoming: RoomMessage[]): RoomMessage[] {
	if (incoming.length === 0) return have;
	const bySeq = new Map(have.map((m) => [m.seq, m]));
	for (const m of incoming) bySeq.set(m.seq, m);
	return [...bySeq.values()].sort((a, b) => a.seq - b.seq);
}

/** The cursor to pass as `after=` next time: the newest seq held, or undefined. */
export function nextCursor(messages: RoomMessage[]): number | undefined {
	return messages.length ? messages[messages.length - 1].seq : undefined;
}

/**
 * How many posts a member has not been given yet. Drives the "behind" hint next
 * to a member that was busy or offline; a room's head is its newest seq.
 */
export function behindBy(head: number, m: RoomMember): number {
	return Math.max(0, head - m.last_delivered_seq);
}

/** Whether `sessionId` may post: a member of a live room, and not an observer. */
export function canPost(room: Room, sessionId: string): boolean {
	if (room.archived) return false;
	const me = room.members.find((m) => m.session_id === sessionId);
	return !!me && me.role !== 'observer';
}

/** Rooms a session is NOT in yet, for the "add to room" menu. */
export function joinableRooms(all: Room[], sessionId: string): Room[] {
	return all.filter((r) => !r.archived && !r.members.some((m) => m.session_id === sessionId));
}
