import { errMessage } from '$lib/api';
import { toasts } from '$lib/toast.svelte';
import { m } from '$lib/paraglide/messages';
import type { ProfileSpecForm } from './profiles';
import type { SpawnForm } from './spawnForm.svelte';

export async function createProfile(
	sf: SpawnForm,
	name: string,
	spec: ProfileSpecForm
): Promise<boolean> {
	try {
		const p = await sf.profileActions.create({ name, ...spec });
		sf.selectedProfileId = p.id;
		sf.oneOff = null;
		return true;
	} catch (e) {
		toasts.error(m.spawn_profile_toast_failed({ error: errMessage(e) }));
		return false;
	}
}

export async function saveProfile(sf: SpawnForm, id: string, name: string, spec: ProfileSpecForm) {
	try {
		await sf.profileActions.update(id, { name, spec });
		toasts.ok(m.spawn_profile_toast_saved());
	} catch (e) {
		toasts.error(m.spawn_profile_toast_failed({ error: errMessage(e) }));
	}
}

export async function deleteProfile(sf: SpawnForm, id: string) {
	try {
		await sf.profileActions.remove(id);
		if (sf.selectedProfileId === id) {
			sf.selectedProfileId = null;
			sf.oneOff = null;
		}
	} catch (e) {
		toasts.error(m.spawn_profile_toast_failed({ error: errMessage(e) }));
	}
}
