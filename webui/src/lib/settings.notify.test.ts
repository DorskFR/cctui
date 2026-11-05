// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { settings } from './settings.svelte';
import { notify } from './notify.svelte';
import { auth } from './auth.svelte';
import { api } from './api';

let put: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
	vi.useFakeTimers();
	auth.isAuthed = true;
	put = vi.spyOn(api, 'put').mockResolvedValue(undefined);
});

afterEach(() => {
	put.mockRestore();
	vi.useRealTimers();
	auth.isAuthed = false;
	notify.permission = 'default';
	localStorage.clear();
});

describe('notifications: one stored value', () => {
	it('records the toggle in the settings blob and nowhere else', async () => {
		notify.permission = 'granted';
		vi.spyOn(notify, 'enable').mockImplementation(async () => {
			notify.enabled = true;
			return true;
		});

		expect(await settings.setNotifyEnabled(true)).toBe(true);
		expect(settings.state.display.notifyEnabled).toBe(true);

		expect(await settings.setNotifyEnabled(false)).toBe(false);
		expect(notify.enabled).toBe(false);
		expect(settings.state.display.notifyEnabled).toBe(false);

		expect(localStorage.getItem('cctui_notify_enabled')).toBeNull();
	});

	it('stores the notification sound in the blob only', () => {
		settings.setNotifySound(false);
		expect(notify.sound).toBe(false);
		expect(settings.state.display.notifySound).toBe(false);
		settings.setNotifySound(true);
		expect(settings.state.display.notifySound).toBe(true);
		expect(localStorage.getItem('cctui_notify_sound')).toBeNull();
	});

	it('only reports enabled while the browser still grants permission', () => {
		notify.permission = 'denied';
		notify.applyPersisted(true, true);
		expect(notify.enabled).toBe(false);
		notify.permission = 'granted';
		notify.applyPersisted(true, false);
		expect(notify.enabled).toBe(true);
		expect(notify.sound).toBe(false);
	});
});
