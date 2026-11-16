// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { CreateNotificationEndpoint } from '@bindings/CreateNotificationEndpoint';
import type { NotificationEndpoint } from '@bindings/NotificationEndpoint';

const query = vi.hoisted(() => ({
	data: undefined as NotificationEndpoint[] | undefined,
	isPending: true
}));
const actions = vi.hoisted(() => ({
	create: vi.fn(async (_body: CreateNotificationEndpoint) => undefined),
	remove: vi.fn(async (_id: string) => undefined),
	test: vi.fn(async (_id: string) => ({ delivery_id: 'd1' }))
}));
vi.mock('$lib/queries', () => ({
	useNotificationEndpoints: () => query,
	useNotificationEndpointActions: () => actions
}));
vi.mock('$lib/components/molecules/PushSubscriptionToggle.svelte', async () => ({
	default: (await import('./PushSubscriptionToggle.stub.svelte')).default
}));

import EndpointsGroup from './EndpointsGroup.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
	vi.clearAllMocks();
});

const endpoint = (over: Partial<NotificationEndpoint>): NotificationEndpoint => ({
	id: 'e1',
	channel: 'ntfy',
	label: null,
	config: { url: 'https://ntfy.example', topic: 'alerts', has_token: true },
	enabled: true,
	created_at: '2026-11-16T00:00:00Z',
	last_ok_at: null,
	...over
});

function render(list: NotificationEndpoint[] | undefined) {
	Object.assign(query, { data: list, isPending: list === undefined });
	comp = mount(EndpointsGroup, { target: document.body, props: {} });
	flushSync();
}

const settle = async () => {
	await new Promise((resolve) => setTimeout(resolve, 0));
	flushSync();
};

function type(id: string, value: string) {
	const el = document.querySelector<HTMLInputElement>(`#${id}`) as HTMLInputElement;
	el.value = value;
	el.dispatchEvent(new Event('input', { bubbles: true }));
	flushSync();
}

const button = (selector: string) =>
	document.querySelector<HTMLButtonElement>(selector) as HTMLButtonElement;
const rows = () => [...document.querySelectorAll<HTMLElement>('[data-journey="notif-endpoint"]')];

describe('notification endpoints list', () => {
	it('mounts this browser push toggle', () => {
		render([]);
		expect(document.querySelector('[data-push-toggle-stub]')).not.toBeNull();
	});

	it('lists endpoints with their target; the server endpoint first and not removable', () => {
		render([
			endpoint({ id: 'e2', channel: 'email', config: { address: 'me@example.org' } }),
			endpoint({ id: 'env', label: 'server', config: { source: 'env' } }),
			endpoint({ id: 'e1' })
		]);
		expect(rows().map((r) => r.dataset.journeyKey)).toEqual(['env', 'e2', 'e1']);
		expect(rows()[1].textContent).toContain('me@example.org');
		expect(rows()[2].textContent).toContain('https://ntfy.example/alerts');
		expect(button('[data-endpoint-remove="env"]')).toBeNull();
		expect(button('[data-endpoint-test="env"]')).not.toBeNull();
	});

	it('adds an ntfy topic with its token and clears the form', async () => {
		render([]);
		const add = button('[data-endpoint-add="ntfy"]');
		expect(add.disabled).toBe(true);
		type('notif-ntfy-url', ' https://ntfy.example ');
		type('notif-ntfy-topic', 'alerts');
		type('notif-ntfy-token', 'tk_secret');
		expect(add.disabled).toBe(false);
		add.click();
		await settle();
		expect(actions.create).toHaveBeenCalledWith({
			channel: 'ntfy',
			label: null,
			config: { url: 'https://ntfy.example', topic: 'alerts', token: 'tk_secret' }
		});
		expect((document.querySelector('#notif-ntfy-topic') as HTMLInputElement).value).toBe('');
	});

	it('adds an email address only once it looks like one', async () => {
		render([]);
		const add = button('[data-endpoint-add="email"]');
		type('notif-email-address', 'not-an-address');
		expect(add.disabled).toBe(true);
		type('notif-email-address', 'me@example.org');
		add.click();
		await settle();
		expect(actions.create).toHaveBeenCalledWith({
			channel: 'email',
			label: null,
			config: { address: 'me@example.org' }
		});
	});

	it('sends a test to that endpoint only', async () => {
		render([endpoint({ id: 'e1' }), endpoint({ id: 'e2' })]);
		button('[data-endpoint-test="e2"]').click();
		await settle();
		expect(actions.test).toHaveBeenCalledTimes(1);
		expect(actions.test).toHaveBeenCalledWith('e2');
	});

	it('removes an endpoint', async () => {
		render([endpoint({ id: 'e1' })]);
		button('[data-endpoint-remove="e1"]').click();
		await settle();
		expect(actions.remove).toHaveBeenCalledWith('e1');
	});
});
