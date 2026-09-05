import type { SessionListItem } from '@bindings/SessionListItem';

export interface LabelAttacher {
	attachLabel: (sessionId: string, labelId: string) => Promise<unknown>;
	listSessions: () => Promise<{ sessions: SessionListItem[] }>;
	sleep?: (ms: number) => Promise<void>;
}

/** Attach labels to a known session; a deleted label or a blip never fails
 *  the spawn that carried it. */
export async function attachLabelsTo(api: LabelAttacher, sessionId: string, ids: string[]) {
	for (const id of ids) {
		try {
			await api.attachLabel(sessionId, id);
		} catch {
			/* best-effort */
		}
	}
}

/** Machine spawns return no session id (the worker registers its own later):
 *  find the newest session on the same (machine, cwd) registered at or after
 *  the request, retrying briefly while the worker comes up. */
export async function attachLabelsToSpawned(
	api: LabelAttacher,
	machineId: string,
	cwd: string,
	sinceMs: number,
	ids: string[]
) {
	if (!ids.length) return;
	const sleep = api.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
	for (let i = 0; i < 6; i++) {
		let list: { sessions: SessionListItem[] };
		try {
			list = await api.listSessions();
		} catch {
			return;
		}
		const match = list.sessions
			.filter((s) => s.machine_id === machineId && s.working_dir === cwd)
			.filter((s) => !s.registered_at || new Date(s.registered_at).getTime() >= sinceMs - 2000)
			.sort(
				(a, b) =>
					new Date(b.registered_at ?? 0).getTime() - new Date(a.registered_at ?? 0).getTime()
			)[0];
		if (match) {
			await attachLabelsTo(api, match.id, ids);
			return;
		}
		await sleep(500);
	}
}
