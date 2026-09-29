// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { PluginInstanceSettings } from '@bindings/PluginInstanceSettings';

const query = vi.hoisted(() => ({
	data: undefined as PluginInstanceSettings | undefined,
	isPending: true,
	isError: false
}));
const endpoints = vi.hoisted(() => ({
	savePluginInstanceSettings: vi.fn(async () => ({})),
	rotatePluginProxySecret: vi.fn(async () => ({ id: 'ghreview', secret: 'new-secret' }))
}));
const copyText = vi.hoisted(() => vi.fn(async () => undefined));
const toasts = vi.hoisted(() => ({ ok: vi.fn(), error: vi.fn(), info: vi.fn() }));

vi.mock('$lib/queries', () => ({
	usePluginInstanceSettings: () => query,
	endpoints,
	qk: {
		plugins: ['plugins'],
		adminPlugins: ['admin', 'plugins'],
		adminPluginSettings: (id: string) => ['admin', 'plugins', id, 'settings']
	}
}));
vi.mock('@tanstack/svelte-query', () => ({
	useQueryClient: () => ({ invalidateQueries: vi.fn(async () => undefined) })
}));
vi.mock('$lib/clipboard', () => ({ copyText }));
vi.mock('$lib/toast.svelte', () => ({ toasts }));

import Form from './PluginInstanceSettingsForm.svelte';

const settings = (over: Partial<PluginInstanceSettings> = {}): PluginInstanceSettings => ({
	id: 'ghreview',
	instance_settings: [
		{ key: 'upstream', label: 'Backend URL', type: 'url', secret: false },
		{ key: 'webhookSecret', label: 'Webhook secret', type: 'string', secret: true }
	],
	values: { upstream: 'https://ghreview.example' },
	secrets_set: { webhookSecret: false },
	backend_upstream_setting: 'upstream',
	proxy_secret_set: false,
	...over
});

let host: HTMLElement;
let component: Record<string, unknown>;

function render() {
	host = document.createElement('div');
	document.body.appendChild(host);
	component = mount(Form, { target: host, props: { pluginId: 'ghreview' } });
	flushSync();
}

function at(journey: string, key?: string): HTMLElement | null {
	const sel = key ? `[data-journey="${journey}"][data-key="${key}"]` : `[data-journey="${journey}"]`;
	return host.querySelector(sel);
}

function field(journey: string, key: string): HTMLInputElement {
	const el = at(journey, key);
	return (el instanceof HTMLInputElement ? el : el!.querySelector('input')!) as HTMLInputElement;
}

function type(input: HTMLInputElement, value: string) {
	input.value = value;
	input.dispatchEvent(new Event('input', { bubbles: true }));
	flushSync();
}

beforeEach(() => {
	document.body.innerHTML = '';
	query.data = undefined;
	query.isPending = true;
	query.isError = false;
	endpoints.savePluginInstanceSettings.mockClear();
	endpoints.rotatePluginProxySecret.mockClear();
	copyText.mockClear();
	vi.spyOn(globalThis, 'confirm').mockReturnValue(true);
});

describe('plugin instance settings form', () => {
	it('shows the declared fields with the stored non-secret values', () => {
		query.data = settings();
		query.isPending = false;
		render();
		expect(field('plugin-instance-field', 'upstream').value).toBe('https://ghreview.example');
		expect(field('plugin-instance-secret', 'webhookSecret').value).toBe('');
		unmount(component);
	});

	it('never seeds a secret field, and says whether one is set', () => {
		query.data = settings({ secrets_set: { webhookSecret: true } });
		query.isPending = false;
		render();
		const secret = field('plugin-instance-secret', 'webhookSecret');
		expect(secret.getAttribute('type')).toBe('password');
		expect(secret.value).toBe('');
		expect(at('plugin-instance-secret-state', 'webhookSecret')?.textContent?.trim()).toBe('set');
		unmount(component);
	});

	it('sends only the keys the admin touched', async () => {
		query.data = settings();
		query.isPending = false;
		render();
		type(field('plugin-instance-secret', 'webhookSecret'), 'hunter2');
		(at('plugin-instance-save') as HTMLButtonElement).click();
		await vi.waitFor(() => expect(endpoints.savePluginInstanceSettings).toHaveBeenCalled());
		expect(endpoints.savePluginInstanceSettings).toHaveBeenCalledWith('ghreview', { webhookSecret: 'hunter2' });
		unmount(component);
	});

	it('cannot save while nothing was typed', () => {
		query.data = settings();
		query.isPending = false;
		render();
		expect((at('plugin-instance-save') as HTMLButtonElement).disabled).toBe(true);
		unmount(component);
	});

	it('clears a secret by writing an empty value', async () => {
		query.data = settings({ secrets_set: { webhookSecret: true } });
		query.isPending = false;
		render();
		(at('plugin-instance-secret-clear', 'webhookSecret') as HTMLButtonElement).click();
		await vi.waitFor(() => expect(endpoints.savePluginInstanceSettings).toHaveBeenCalled());
		expect(endpoints.savePluginInstanceSettings).toHaveBeenCalledWith('ghreview', { webhookSecret: '' });
		unmount(component);
	});

	it('shows a rotated proxy secret once, and only for a plugin with a backend', async () => {
		query.data = settings();
		query.isPending = false;
		render();
		expect(at('plugin-instance-proxy')).not.toBeNull();
		(at('plugin-instance-proxy-rotate') as HTMLButtonElement).click();
		await vi.waitFor(() => expect(at('plugin-instance-proxy-secret-value')).not.toBeNull());
		expect(at('plugin-instance-proxy-secret-value')?.textContent).toContain('new-secret');
		unmount(component);
	});

	it('offers no proxy section to a plugin without a backend', () => {
		query.data = settings({ backend_upstream_setting: null });
		query.isPending = false;
		render();
		expect(at('plugin-instance-proxy')).toBeNull();
		unmount(component);
	});

	it('says so when the plugin declares nothing', () => {
		query.data = settings({ instance_settings: [], backend_upstream_setting: null });
		query.isPending = false;
		render();
		expect(at('plugin-instance-empty')).not.toBeNull();
		expect(at('plugin-instance-save')).toBeNull();
		unmount(component);
	});
});
