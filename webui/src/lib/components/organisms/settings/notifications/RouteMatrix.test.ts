// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { NotificationSettings } from '@bindings/NotificationSettings';
import { api } from '$lib/api';
import { auth } from '$lib/auth.svelte';
import { DEFAULT_NOTIFICATIONS } from '$lib/notificationPrefs';
import { settings } from '$lib/settings.svelte';
import RouteMatrix from './RouteMatrix.svelte';

type PutBody = { data: { notifications: NotificationSettings } };

let comp: ReturnType<typeof mount> | null = null;
let put: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
	auth.isAuthed = false;
	localStorage.clear();
	settings.setNotifications(DEFAULT_NOTIFICATIONS);
	vi.useFakeTimers();
	auth.isAuthed = true;
	put = vi.spyOn(api, 'put').mockResolvedValue(undefined);
});

afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
	put.mockRestore();
	vi.useRealTimers();
	auth.isAuthed = false;
	localStorage.clear();
});

function render(resolved?: NotificationSettings) {
	comp = mount(RouteMatrix, { target: document.body, props: { resolved } });
	flushSync();
}

const stageSelect = (kind: string, channel: string) =>
	document.querySelector<HTMLSelectElement>(`#notif-stage-${kind}-${channel}`) as HTMLSelectElement;
const minutesInput = (kind: string, channel: string) =>
	document.querySelector<HTMLInputElement>(`#notif-minutes-${kind}-${channel}`);

function change(el: HTMLSelectElement | HTMLInputElement, value: string) {
	el.value = value;
	el.dispatchEvent(new Event('change', { bubbles: true }));
	flushSync();
}

function sentNotifications(): NotificationSettings {
	settings.flush();
	expect(put).toHaveBeenCalled();
	const [path, body] = put.mock.calls.at(-1) as [string, PutBody];
	expect(path).toBe('/settings');
	return body.data.notifications;
}

describe('notification route matrix', () => {
	it('renders one cell per kind and channel with the resolved stage', () => {
		render({
			...DEFAULT_NOTIFICATIONS,
			routes: { approval: { ntfy: { mode: 'now' } } }
		});
		expect(document.querySelectorAll('[data-stage-cell]')).toHaveLength(7 * 4);
		expect(stageSelect('approval', 'ntfy').value).toBe('now');
		expect(stageSelect('approval', 'web_push').value).toBe('after');
		expect(minutesInput('approval', 'web_push')?.value).toBe('5');
		expect(stageSelect('user_action', 'in_app').value).toBe('now');
		expect(stageSelect('user_action', 'ntfy').value).toBe('off');
	});

	it('changing a cell PUTs a sparse override under notifications.routes', () => {
		render();
		change(stageSelect('needs_input', 'ntfy'), 'now');
		expect(sentNotifications()).toEqual({
			...DEFAULT_NOTIFICATIONS,
			routes: { needs_input: { ntfy: { mode: 'now' } } }
		});
	});

	it('switching to "after" then editing minutes PUTs the clamped minutes', () => {
		render();
		change(stageSelect('session_stuck', 'email'), 'after');
		const minutes = minutesInput('session_stuck', 'email');
		expect(minutes?.value).toBe('10');
		change(minutes as HTMLInputElement, '9999');
		expect(sentNotifications().routes).toEqual({
			session_stuck: { email: { mode: 'after', minutes: 1440 } }
		});
	});

	it('disables a column whose channel is off and every cell when the master switch is off', () => {
		render({ ...DEFAULT_NOTIFICATIONS, channels: { email: { enabled: false } } });
		expect(stageSelect('approval', 'email').disabled).toBe(true);
		expect(stageSelect('approval', 'ntfy').disabled).toBe(false);
		settings.setNotifications({ enabled: false });
		flushSync();
		expect(stageSelect('approval', 'ntfy').disabled).toBe(true);
	});

	it.each([[[]], [{}], [{ channels: {} }]])('renders the defaults when the resolved view is %j', (resolved) => {
		render(resolved as unknown as NotificationSettings);
		expect(document.querySelectorAll('[data-stage-cell]')).toHaveLength(7 * 4);
		expect(stageSelect('approval', 'in_app').value).toBe('now');
		expect(stageSelect('approval', 'web_push').value).toBe('after');
	});
});
