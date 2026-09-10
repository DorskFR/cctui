import { goto } from '$app/navigation';
import type { Journey } from '@dorsk/journey';
import {
	DONE_PREFIX,
	type JourneyApi,
	type JourneyStorage,
	mount,
	PROGRESS_KEY,
	type Strings
} from '@dorsk/journey/runtime';
import journeys from './journeys.generated.json';
import { m } from './paraglide/messages';
import { settings } from './settings.svelte';

/** A spec may name a message id instead of carrying the copy. An id with no
 *  message resolves to the id itself upstream, which is ugly but readable —
 *  better than a blank card. */
export function translate(id: string): string | undefined {
	const message = (m as Record<string, unknown>)[id];
	return typeof message === 'function' ? (message() as string) : undefined;
}

/** The runtime interpolates `{i}`/`{n}`/`{route}` itself, so paraglide has to
 *  hand back the braces rather than fill them in. */
export function strings(): Partial<Strings> {
	return {
		next: m.journey_next(),
		exit: m.journey_exit(),
		step: m.journey_step({ i: '{i}', n: '{n}' }),
		goToPage: m.journey_go_to_page(),
		goToPageBody: m.journey_go_to_page_body({ route: '{route}' }),
		goToPageAction: m.journey_go_to_page_action(),
		press: m.journey_press()
	};
}

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
			navigate: (route) => goto(route),
			translate,
			strings
		});
		await api.register(journeys as Journey[]);
		watchLocale(api);
	})();
	return mounted;
}

/** Copy and chrome are resolved when a card is drawn, so a language switch only
 *  reaches an open guide by drawing it again at the same step. `lang` is the
 *  locale the runtime itself reads, so it is the signal worth following. */
function watchLocale(api: JourneyApi): void {
	const root = document.documentElement;
	let lang = root.lang;
	new MutationObserver(() => {
		if (root.lang === lang) return;
		lang = root.lang;
		const current = api.current();
		if (current) void api.start(current.id, { mode: 'guide', from: current.index });
	}).observe(root, { attributeFilter: ['lang'] });
}
