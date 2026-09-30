// A session-id → rooms index for the session cards.
//
// Deliberately NOT part of the session-list payload: membership changes far less
// often than a session row, and `/rooms` already returns every room with its
// members, so one request answers for the whole page. Refreshed on the ws room
// tick by whoever mounts it.
import { listRooms, type Room, type SessionRoom } from './rooms';

/** Invert rooms-with-members into the per-session view the cards need. */
export function indexBySession(rooms: Room[]): Map<string, SessionRoom[]> {
	const out = new Map<string, SessionRoom[]>();
	for (const room of rooms) {
		if (room.archived) continue;
		for (const mem of room.members) {
			const list = out.get(mem.session_id) ?? [];
			list.push({ id: room.id, name: room.name, role: mem.role });
			out.set(mem.session_id, list);
		}
	}
	return out;
}

class RoomIndex {
	private index = $state(new Map<string, SessionRoom[]>());
	/** Every live room, for the "add to room" picker. */
	rooms = $state<Room[]>([]);
	private loading = false;

	/** Rooms `sessionId` is in; empty until the first load resolves. */
	for(sessionId: string): SessionRoom[] {
		return this.index.get(sessionId) ?? [];
	}

	/** Load once per caller burst. A failure is silent: a missing badge is not
	 *  worth a toast on a page the user did not open for rooms. */
	async load() {
		if (this.loading) return;
		this.loading = true;
		try {
			const rooms = await listRooms();
			this.rooms = rooms;
			this.index = indexBySession(rooms);
		} catch {
			// leave the previous index in place
		} finally {
			this.loading = false;
		}
	}
}

export const roomIndex = new RoomIndex();
