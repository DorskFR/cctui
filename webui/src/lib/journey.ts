import { goto } from '$app/navigation';
import type { Journey } from '@dorsk/journey';
import {
	docPresenter,
	DONE_PREFIX,
	guidePresenter,
	nonePresenter,
	type JourneyApi,
	type JourneyStorage,
	mount,
	type Presenter,
	PROGRESS_KEY,
	translator
} from '@dorsk/journey/runtime';
import journeys from './journeys.generated.json';
import { settings } from './settings.svelte';
import { type DeckCard, deckPresenter } from './welcomeDeck.svelte';

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

/** The running journey's steps as deck cards. The presenter is handed one step at
 *  a time, but a carousel needs the whole deck up front. */
function deckCards(api: JourneyApi): DeckCard[] {
	const engine = api.engine();
	if (!engine) return [];
	return engine.ir.steps.map((step) => ({
		title: engine.text(step.say?.title) ?? '',
		body: engine.text(step.say?.body) ?? ''
	}));
}

function markSeen(api: JourneyApi): void {
	const ir = api.engine()?.ir;
	if (!ir) return;
	settingsStorage.set(`${DONE_PREFIX}${ir.id}@${ir.version}`, '1');
}

let mounted: Promise<void> | null = null;

/** Mount the runtime once and register the bundled journeys. Waits for the
 *  server copy of the settings first, so a resumed tour reads the blob rather
 *  than the local cache. */
export function mountJourneys(): Promise<void> {
	mounted ??= (async () => {
		await settings.load();
		let api: JourneyApi | null = null;
		let overlay: Presenter | null = null;
		const host = () => {
			if (!api) throw new Error('journey runtime is not mounted yet');
			return api;
		};
		const fallback = () =>
			(overlay ??= guidePresenter(host().overlay, translator(() => host().strings())));
		const deck = deckPresenter({
			cards: () => deckCards(host()),
			markSeen: () => markSeen(host()),
			fallback
		});
		api = mount({
			storage: settingsStorage,
			navigate: (route) => goto(route),
			presenter: (name) => (name === 'guide' ? deck : name === 'doc' ? docPresenter(host().overlay) : nonePresenter)
		});
		await api.register(journeys as Journey[]);
	})();
	return mounted;
}
