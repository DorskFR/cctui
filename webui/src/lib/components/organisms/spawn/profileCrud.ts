import { errMessage } from '$lib/api';
import { toasts } from '$lib/toast.svelte';
import { m } from '$lib/paraglide/messages';
import { specFromForm, uniqueProfileName, type ProfileSpecForm } from './profiles';
import type { SpawnForm } from './spawnForm.svelte';

export async function createProfile(sf: SpawnForm) {
	const base = sf.profileSpec ?? specFromForm(sf.form, sf.allAccounts, sf.allPools);
	const name = uniqueProfileName(
		m.spawn_profile_new_name(),
		sf.profiles.map((p) => p.name)
	);
	try {
		const p = await sf.profileActions.create({ name, ...base });
		sf.selectedProfileId = p.id;
		sf.oneOff = null;
	} catch (e) {
		toasts.error(m.spawn_profile_toast_failed({ error: errMessage(e) }));
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
