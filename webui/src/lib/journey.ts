import { goto } from '$app/navigation';
import type { Journey } from '@dorsk/journey';
import { DONE_PREFIX, type JourneyStorage, mount, PROGRESS_KEY } from '@dorsk/journey/runtime';
import journeys from './journeys.generated.json';
import { settings } from './settings.svelte';

/** Progress and the autostart-once marker live in the user's settings blob so a
 *  tour resumes from any browser the user signs in from. */
export const settingsStorage: JourneyStorage = {
	get(key) {
		if (key === PROGRESS_KEY) return settings.onboarding.progress;
		const done = parseDoneKey(key);
		if (!done) return null;
		return settings.onboarding.seenVersion[done.id] === done.version ? '1' : null;
	},
	set(key, value) {
		if (key === PROGRESS_KEY) {
			settings.setOnboarding({ progress: value });
			return;
		}
		const done = parseDoneKey(key);
		if (!done) return;
		settings.setOnboarding({
			seenVersion: { ...settings.onboarding.seenVersion, [done.id]: done.version }
		});
	},
	remove(key) {
		if (key === PROGRESS_KEY) {
			settings.setOnboarding({ progress: null });
			return;
		}
		const done = parseDoneKey(key);
		if (!done) return;
		const { [done.id]: _, ...rest } = settings.onboarding.seenVersion;
		settings.setOnboarding({ seenVersion: rest });
	}
};

/** `journey:done:<id>@<version>` → its parts, or null for any other key. */
export function parseDoneKey(key: string): { id: string; version: number } | null {
	if (!key.startsWith(DONE_PREFIX)) return null;
	const at = key.lastIndexOf('@');
	if (at < DONE_PREFIX.length) return null;
	const version = Number(key.slice(at + 1));
	if (!Number.isInteger(version)) return null;
	return { id: key.slice(DONE_PREFIX.length, at), version };
}

let mounted: Promise<void> | null = null;

/** Mount the runtime once and register the bundled journeys. Waits for the
 *  server copy of the settings first, so a resumed tour reads the blob rather
 *  than the local cache. */
export function mountJourneys(): Promise<void> {
	mounted ??= (async () => {
		await settings.load();
		const api = mount({
			storage: settingsStorage,
			navigate: (route) => goto(route)
		});
		await api.register(journeys as Journey[]);
	})();
	return mounted;
}
