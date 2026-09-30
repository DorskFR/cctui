// Room client: the REST calls plus the pure helpers the picker and the card
// badge share. A room is a field on a session plus a permission boundary, so
// there is no timeline or composer here — `CctuiRoom` is the only broadcast, and
// it is an agent-side tool.
import { api } from './api';

export interface RoomMember {
	session_id: string;
	name: string | null;
	adapter: string | null;
	machine: string | null;
	state: 'live' | 'ended' | 'archived';
	last_delivered_seq: number;
}

export interface Room {
	id: string;
	name: string;
	archived: boolean;
	members: RoomMember[];
}

export const listRooms = () => api.get<{ rooms: Room[] }>('/rooms').then((r) => r.rooms);

export const renameRoom = (id: string, name: string) => api.patch<Room>(`/rooms/${id}`, { name });

export const setRoomArchived = (id: string, archived: boolean) =>
	api.patch<Room>(`/rooms/${id}`, { archived });

export const deleteRoom = (id: string) => api.del<void>(`/rooms/${id}`);

/** Put a session in an existing room. */
export const setSessionRoom = (sessionId: string, roomId: string) =>
	api.put<{ room_id: string; name: string }>(`/sessions/${sessionId}/room`, { room_id: roomId });

/** Put a session in a room named `name`, creating it if it does not exist. */
export const setSessionRoomByName = (sessionId: string, name: string) =>
	api.put<{ room_id: string; name: string }>(`/sessions/${sessionId}/room`, { name });

export const clearSessionRoom = (sessionId: string) =>
	api.del<void>(`/sessions/${sessionId}/room`);

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

/** Live rooms, newest first, for the picker. */
export const pickable = (all: Room[]): Room[] => all.filter((r) => !r.archived);

/**
 * Whether `name` would land in an existing room rather than create one. The
 * server matches case-insensitively, so the picker must too or it offers
 * "create" for a name that will silently reuse.
 */
export function matchByName(all: Room[], name: string): Room | undefined {
	const want = name.trim().toLowerCase();
	if (!want) return undefined;
	return all.find((r) => r.name.trim().toLowerCase() === want);
}

/** Whether typing `name` should offer a create row. */
export function canCreate(all: Room[], name: string): boolean {
	return name.trim().length > 0 && !matchByName(all, name);
}
