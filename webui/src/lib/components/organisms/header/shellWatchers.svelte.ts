import { untrack } from 'svelte';
import { useQueryClient } from '@tanstack/svelte-query';
import type { SessionListResponse } from '@bindings/SessionListResponse';
import { ws } from '#lib/ws.svelte.ts';
import { useSessions, useVersion, qk } from '#lib/queries/index.ts';
import { setUploadCaps } from '#lib/uploadCaps.svelte.ts';
import { fontScale } from '#lib/fontscale.svelte.ts';
import { notify } from '#lib/notify.svelte.ts';
import { notifications } from '#lib/notifications.svelte.ts';
import { settings } from '#lib/settings.svelte.ts';

/** Must be called during component init, inside the query provider. */
export function installShellWatchers() {
	const version = useVersion();
	const qc = useQueryClient();
	const sessions = useSessions(() => false);

	$effect(() => notify.setInstanceName(version.data?.instance_name ?? null));
	$effect(() => setUploadCaps(version.data?.upload_caps));

	$effect(() => {
		void ws.changeTick;
		qc.invalidateQueries({ queryKey: qk.sessionsAll });
	});

	// Patched in place on both list caches; the 60s poll reconciles the rest.
	$effect(() =>
		ws.onListPatch((p) => {
			const { session_id, ...fields } = p;
			for (const key of [qk.sessions(false), qk.sessions(true), qk.sessions(false, true)]) {
				qc.setQueryData<SessionListResponse>(key, (old) =>
					old
						? {
								...old,
								sessions: old.sessions.map((s) => (s.id === session_id ? { ...s, ...fields } : s))
							}
						: old
				);
			}
		})
	);

	const needsInput = $derived(
		(sessions.data?.sessions ?? []).filter((s) => s.attention === 'needs_input')
	);
	const covered = $derived(
		new Set(notifications.items.flatMap((n) => (n.session_id ? [n.session_id] : [])))
	);

	$effect(() => notifications.connect(ws));
	$effect(() => ws.onNotification((n) => notify.announce(n)));
	$effect(() => notify.setUnread(notifications.unread));
	// A socket that was down missed frames: every (re)open reloads the list.
	$effect(() => {
		if (ws.status !== 'open') return;
		untrack(() => void notifications.load().catch(() => {}));
	});
	$effect(() => notify.reconcile(needsInput, covered));
	$effect(() => notify.reconcileCompleted(sessions.data?.sessions));

	$effect(() => {
		const f = fontScale.current;
		if (settings.state.display.fontScale !== f) settings.setDisplay({ fontScale: f });
	});
}
