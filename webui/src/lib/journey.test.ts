import { beforeEach, describe, expect, it } from 'vitest';
import { DONE_PREFIX, PROGRESS_KEY } from '@dorsk/journey/runtime';
import { parseDoneKey, settingsStorage } from './journey';
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
