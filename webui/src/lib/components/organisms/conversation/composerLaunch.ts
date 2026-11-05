import { goto } from '$app/navigation';
import { page } from '$app/state';
import type { SessionListItem } from '@bindings/SessionListItem';
import { errMessage } from '$lib/api';
import { BRIEF_FETCH_MAX_BYTES, launchFollowup, type FollowupLaunchDeps } from '$lib/followup';
import { m } from '$lib/paraglide/messages';
import { pendingSpawns } from '$lib/pendingSpawns.svelte';
import { endpoints } from '$lib/queries';
import { sessionHref } from '$lib/sessionFailureToast';
import { toasts } from '$lib/toast.svelte';
import { ws } from '$lib/ws.svelte';
import { findSpawnedSession } from '$lib/components/organisms/spawn/spawnMatch';

export type LaunchActions = Pick<FollowupLaunchDeps, 'spawn' | 'archive'>;

/** Start the follow-up with `text` as its instruction and archive `source`.
 * Resolves `true` once the draft may be cleared. */
export async function launchFromComposer(
	source: SessionListItem,
	text: string,
	actions: LaunchActions
): Promise<boolean> {
	const out = await launchFollowup(source, text, {
		brief: (id) => endpoints.brief(id, BRIEF_FETCH_MAX_BYTES),
		spawn: (body, files) => actions.spawn(body, files),
		awaitSpawn: async (commandId, sessionId) => {
			let found = sessionId;
			const outcome = await ws.awaitSpawn(commandId, sessionId, {
				probe: async () => {
					const { sessions } = await endpoints.sessions(false);
					const hit = findSpawnedSession(sessions, source.machine_id, commandId, found);
					if (hit) found = hit.id;
					return hit;
				}
			});
			return { ...outcome, sessionId: found };
		},
		archive: (id) => actions.archive(id)
	});
	if (out.status === 'failed') {
		toasts.error(
			m.spawn_toast_spawn_failed({
				error: out.error == null ? m.spawn_error_unknown() : errMessage(out.error)
			})
		);
		return false;
	}
	if (out.status === 'unconfirmed') {
		pendingSpawns.add({
			commandId: out.commandId,
			sessionId: out.sessionId,
			machineId: source.machine_id,
			name: '',
			workingDir: source.working_dir
		});
		toasts.info(m.composer_followup_unconfirmed());
		return true;
	}
	if (out.archiveError !== undefined) {
		toasts.error(m.composer_followup_archive_failed({ error: errMessage(out.archiveError) }));
	} else {
		toasts.ok(m.composer_followup_launched());
	}
	if (out.sessionId)
		void goto(sessionHref(out.sessionId), {
			replaceState: true,
			state: { ...page.state, spawned: out.sessionId } as App.PageState
		});
	return true;
}
