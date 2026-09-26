import { ApiError, errMessage } from '$lib/api';
import type { SpawnRequest } from '@bindings/SpawnRequest';
import { ws, type SpawnProbeHit } from '$lib/ws.svelte';
import { toasts } from '$lib/toast.svelte';
import {
	drafts,
	LAST_MACHINE,
	LAST_SPAWN_NAME,
	LAST_SPAWN_LABELS,
	FOLLOWUP_ARCHIVE_SOURCE,
	normalizeDir
} from '$lib/drafts';
import { machineMemoryKey, dispatchMemoryKey, entryFromForm } from '$lib/spawnMemory';
import { settings } from '$lib/settings.svelte';
import { m } from '$lib/paraglide/messages';
import { buildDispatchBody } from './dispatchBody';
import { attachLabelsTo } from './labelAttach';
import { envMap } from './spawnBody';
import type { SpawnForm } from './spawnForm.svelte';

/** Server autosave: the form mirrored to its draft row (created on first
 * save, updated in place after). False when it can't be a draft yet. */
export async function autosave(sf: SpawnForm): Promise<boolean> {
	sf.cancelAutosave();
	if (sf.busy || sf.autosaving || !sf.autosaveReady) return false;
	sf.autosaving = true;
	try {
		const body = sf.draftBody();
		if (sf.draftId) {
			try {
				await sf.actions.updateDraft(sf.draftId, body);
				return true;
			} catch (e) {
				if (!(e instanceof ApiError && e.status === 404)) throw e;
				sf.draftId = null;
			}
		}
		const res = await sf.actions.spawn({ ...body, save_draft: true }, []);
		sf.draftId = String(res.command_id);
		return true;
	} catch (e) {
		toasts.error(m.spawn_toast_save_draft_failed({ error: errMessage(e) }));
		return false;
	} finally {
		sf.autosaving = false;
	}
}

export async function saveDraft(sf: SpawnForm) {
	sf.cancelAutosave();
	const body = sf.draftBody();
	if (sf.draftId) await sf.actions.updateDraft(sf.draftId, body);
	else await sf.actions.spawn({ ...body, save_draft: true }, []);
	drafts.set(LAST_MACHINE, sf.form.machine_id);
	drafts.set(LAST_SPAWN_NAME, sf.form.name.trim());
	toasts.ok(m.spawn_toast_saved_draft());
	sf.finish();
}

async function spawnProbe(sf: SpawnForm, sessionId: string): Promise<SpawnProbeHit | null> {
	const { sessions } = await sf.labelApi.listSessions();
	return sessions.find((s) => s.id === sessionId) ?? null;
}

export async function spawnOnMachine(sf: SpawnForm) {
	sf.cancelAutosave();
	sf.spawnFailure = null;
	const body: SpawnRequest = sf.buildSpawnBody();
	const labelIds = [...sf.form.labels];
	const memoryCwd = normalizeDir(sf.form.working_dir.trim());
	const memoryMachine = sf.form.machine_id;
	const profile = sf.selectedProfile;
	const res = await sf.actions.spawn(body, sf.files);
	if (res.account) toasts.info(m.spawn_toast_bound_account({ account: res.account }));
	drafts.set(LAST_MACHINE, sf.form.machine_id);
	drafts.set(LAST_SPAWN_NAME, sf.form.name.trim());
	drafts.set(LAST_SPAWN_LABELS, labelIds.join(','));
	// Saved on submit (not on confirmed success) so a slow spawn still
	// records the operator's intent.
	settings.rememberSpawn(machineMemoryKey(memoryMachine, memoryCwd), {
		...entryFromForm(sf.effectiveForm),
		profile_id: profile?.id
	});
	sf.rememberProfileUse(profile);
	toasts.info(m.spawn_toast_spawning());
	const sessionId = res.session_id ?? null;
	const result = await ws.awaitSpawn(res.command_id, sessionId, {
		probe: sessionId ? () => spawnProbe(sf, sessionId) : undefined
	});
	if (sf.followupParent) drafts.set(FOLLOWUP_ARCHIVE_SOURCE, sf.archiveSource ? '1' : '');
	if (result.ok) {
		toasts.ok(m.spawn_toast_spawned());
		if (sf.followupParent && sf.archiveSource) void sf.actions.archive(sf.followupParent);
		sf.discardMirror();
		sf.finish();
	} else if (result.timedOut) {
		// No confirmation ≠ failed: cold spawns routinely land after the wait.
		// Keep the draft so a real miss is one re-open away; re-submitting
		// blindly would dispatch a second agent.
		toasts.info(m.spawn_toast_unconfirmed());
		sf.onspawned();
		sf.onclose();
	} else {
		sf.spawnFailure = result.error ?? m.spawn_error_unknown();
		toasts.error(m.spawn_toast_spawn_failed({ error: sf.spawnFailure }));
	}
}

export async function dispatchToK8s(sf: SpawnForm) {
	// Stable across retries so the server's idempotency dedup makes a
	// re-submit a genuine retry, not a second pod. Cleared on success.
	sf.pendingDispatchId ??= crypto.randomUUID();
	const body = buildDispatchBody(sf.form, envMap(sf.envRows), sf.dispatchProvider, sf.pendingDispatchId);
	const labelIds = [...sf.form.labels];
	const dispatchedId = sf.pendingDispatchId;
	const res = await sf.actions.dispatch(body);
	drafts.set(LAST_SPAWN_NAME, sf.form.name.trim());
	settings.rememberSpawn(dispatchMemoryKey(sf.form.dispatcher, sf.form.repo), entryFromForm(sf.form));
	drafts.set(LAST_SPAWN_LABELS, labelIds.join(','));
	void attachLabelsTo(sf.labelApi, dispatchedId, labelIds);
	toasts.ok(m.spawn_toast_dispatched({ dispatcher: res.dispatcher, handle: res.handle }));
	sf.pendingDispatchId = null;
	sf.discardMirror();
	sf.finish();
}
