import { beforeEach, describe, expect, it } from 'vitest';
import { DONE_PREFIX, PROGRESS_KEY } from '@dorsk/journey/runtime';
import type { Journey } from '@dorsk/journey';
import { resolveText } from '@dorsk/journey/runtime';
import journeys from './journeys.generated.json';
import { locale } from './locale.svelte';
import { parseDoneKey, settingsStorage, strings, translate } from './journey';
import { auth } from './auth.svelte';
import { mergeDefaults, settings } from './settings.svelte';

const KEY = 'cctui_settings';

function blob() {
	return mergeDefaults(JSON.parse(localStorage.getItem(KEY) ?? 'null')).onboarding;
}

beforeEach(() => {
	auth.isAuthed = false;
	localStorage.clear();
	settings.setOnboarding({ seenVersion: {}, progress: null });
});

describe('parseDoneKey', () => {
	it('splits the runtime done marker into id and version', () => {
		expect(parseDoneKey(`${DONE_PREFIX}sessions-list@3`)).toEqual({ id: 'sessions-list', version: 3 });
		expect(parseDoneKey(`${DONE_PREFIX}a@b@2`)).toEqual({ id: 'a@b', version: 2 });
	});
	it('rejects anything else', () => {
		expect(parseDoneKey(PROGRESS_KEY)).toBeNull();
		expect(parseDoneKey(`${DONE_PREFIX}sessions-list`)).toBeNull();
		expect(parseDoneKey(`${DONE_PREFIX}sessions-list@x`)).toBeNull();
	});
});

describe('settingsStorage', () => {
	it('keeps progress in data.onboarding.progress', async () => {
		const record = JSON.stringify({ id: 'sessions-list', version: 1, index: 1 });
		await settingsStorage.set(PROGRESS_KEY, record);
		expect(blob().progress).toBe(record);
		expect(await settingsStorage.get(PROGRESS_KEY)).toBe(record);
		await settingsStorage.remove(PROGRESS_KEY);
		expect(blob().progress).toBeNull();
		expect(await settingsStorage.get(PROGRESS_KEY)).toBeNull();
	});

	it('keeps the done marker as seenVersion[id], matched on the exact version', async () => {
		const key = `${DONE_PREFIX}sessions-list@2`;
		expect(await settingsStorage.get(key)).toBeNull();
		await settingsStorage.set(key, '1');
		expect(blob().seenVersion).toEqual({ 'sessions-list': 2 });
		expect(await settingsStorage.get(key)).toBe('1');
		expect(await settingsStorage.get(`${DONE_PREFIX}sessions-list@3`)).toBeNull();
		await settingsStorage.remove(key);
		expect(blob().seenVersion).toEqual({});
	});

	it('ignores keys it does not own', async () => {
		await settingsStorage.set('journey:other', 'x');
		expect(await settingsStorage.get('journey:other')).toBeNull();
		expect(blob()).toEqual({ seenVersion: {}, progress: null });
	});
});

describe('journey copy and chrome follow the active locale', () => {
	const ir = journeys as unknown as Journey[];
	const texts = (loc: string) =>
		ir.flatMap((journey) => {
			const step = (t: unknown) => resolveText(t as never, translate, loc);
			return [
				step(journey.title),
				step(journey.description),
				...(journey.steps ?? []).flatMap((s) => [step(s.say?.title), step(s.say?.body)])
			];
		});

	it('renders a message id that has no message as the id, never a blank card', () => {
		expect(translate('journey_next')).toBe('Next');
		expect(translate('no_such_message_at_all')).toBeUndefined();
		expect(resolveText({ $msg: 'no_such_message_at_all' }, translate, 'en')).toBe(
			'no_such_message_at_all'
		);
	});

	it('hands the runtime its own placeholders back rather than filling them in', () => {
		const s = strings();
		expect(s.step).toContain('{i}');
		expect(s.step).toContain('{n}');
		expect(s.goToPageBody).toContain('{route}');
	});

	it('localises the library chrome', () => {
		locale.set('en');
		expect(strings().next).toBe('Next');
		locale.set('fr');
		expect(strings().next).toBe('Suivant');
		expect(strings().step).toContain('{i}');
		locale.set('en');
	});

	it('leaves no card blank in either locale', () => {
		for (const loc of ['en', 'fr'])
			for (const t of texts(loc)) expect(t === undefined || t.length > 0).toBe(true);
	});

	it('says something different in French', () => {
		const en = texts('en');
		const fr = texts('fr');
		expect(fr.filter((t, i) => t !== en[i]).length).toBeGreaterThan(0);
	});
});
