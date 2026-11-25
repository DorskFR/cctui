// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { notify } from './notify.svelte';
import { settings } from './settings.svelte';

describe('the notification switch has one stored value', () => {
	beforeEach(() => {
		vi.restoreAllMocks();
		notify.enabled = false;
		settings.setDisplay({ notifyEnabled: false });
	});

	it('stores ON only once the browser granted the permission', async () => {
		vi.spyOn(notify, 'enable').mockImplementation(async () => {
			notify.enabled = true;
			return true;
		});
		expect(await settings.setNotifyEnabled(true)).toBe(true);
		expect(settings.state.display.notifyEnabled).toBe(true);
		expect(settings.notifyEnabled).toBe(true);
	});

	it('keeps it OFF when the permission is refused', async () => {
		vi.spyOn(notify, 'enable').mockImplementation(async () => false);
		expect(await settings.setNotifyEnabled(true)).toBe(false);
		expect(settings.state.display.notifyEnabled).toBe(false);
		expect(settings.notifyEnabled).toBe(false);
	});

	it('turning it OFF clears the stored value and the notifier together', async () => {
		vi.spyOn(notify, 'enable').mockImplementation(async () => {
			notify.enabled = true;
			return true;
		});
		await settings.setNotifyEnabled(true);
		await settings.setNotifyEnabled(false);
		expect(settings.state.display.notifyEnabled).toBe(false);
		expect(notify.enabled).toBe(false);
		expect(settings.notifyEnabled).toBe(false);
	});

	it('reads OFF when this browser lost the permission, whatever is stored', () => {
		settings.setDisplay({ notifyEnabled: true });
		notify.enabled = false;
		expect(settings.notifyEnabled).toBe(false);
	});

	it('leaves no copy of its own in localStorage', async () => {
		vi.spyOn(notify, 'enable').mockImplementation(async () => {
			notify.enabled = true;
			return true;
		});
		await settings.setNotifyEnabled(true);
		settings.setNotifySound(false);
		notify.disable();
		expect(localStorage.getItem('cctui_notify_enabled')).toBeNull();
		expect(localStorage.getItem('cctui_notify_sound')).toBeNull();
	});
});
