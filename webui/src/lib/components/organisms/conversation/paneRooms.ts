import type { QueryClient } from '@tanstack/svelte-query';
import { errMessage } from '$lib/api';
import { qk } from '$lib/queries';
import { clearSessionRoom, setSessionRoom, setSessionRoomByName } from '$lib/rooms';
import { toasts } from '$lib/toast.svelte';

export type RoomPick = { id: string } | { name: string };

// Room is a field on the session, so a change invalidates the session list
// that the grouping and the card badge read.
export function paneRooms(qc: QueryClient) {
	const refresh = () => void qc.invalidateQueries({ queryKey: qk.sessionsAll });
	return {
		set: async (sessionId: string, pick: RoomPick) => {
			try {
				if ('id' in pick) await setSessionRoom(sessionId, pick.id);
				else await setSessionRoomByName(sessionId, pick.name);
				refresh();
			} catch (e) {
				toasts.error(errMessage(e));
			}
		},
		clear: async (sessionId: string) => {
			try {
				await clearSessionRoom(sessionId);
				refresh();
			} catch (e) {
				toasts.error(errMessage(e));
			}
		}
	};
}
