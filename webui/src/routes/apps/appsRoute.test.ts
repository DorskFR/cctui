// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { PluginInfo } from '$lib/plugins/types';

const state = vi.hoisted(() => ({
	url: new URL('https://app.test/apps/demo'),
	params: { id: 'demo', path: '' } as Record<string, string>
}));
const query = vi.hoisted(() => ({ data: undefined as PluginInfo[] | undefined, isPending: true, isError: false }));
const enabled = vi.hoisted(() => ({ value: {} as Record<string, boolean> }));
const goto = vi.hoisted(() => vi.fn(async () => undefined));
const loaded = vi.hoisted(() => ({ page: undefined as unknown, mounted: [] as unknown[] }));

vi.mock('$app/state', () => ({ page: state }));
vi.mock('$app/navigation', () => ({ goto }));
vi.mock('$app/environment', () => ({ browser: true }));
vi.mock('$lib/queries', () => ({
	usePlugins: () => query,
	useMe: () => ({ data: { role: 'admin', user_id: 'u1', user_name: 'dorsk' } })
}));
vi.mock('$lib/toast.svelte', () => ({ toasts: { ok: vi.fn(), info: vi.fn(), error: vi.fn() } }));
vi.mock('$lib/settings.svelte', () => ({
	settings: {
		get pluginsEnabled() {
			return enabled.value;
		}
	}
}));
vi.mock('$lib/plugins/discovery', async (orig) => {
	const real = (await orig()) as Record<string, unknown>;
	return { ...real, loadPluginModule: vi.fn(async () => ({ cctuiApi: 1, page: loaded.page })) };
});

import Route from './[id]/[...path]/+page.svelte';
import { pluginLoader } from '$lib/plugins/loader.svelte';

const info = (over: Partial<PluginInfo> = {}): PluginInfo => ({
	id: 'demo',
	name: 'Demo',
	description: '',
	version: '1.0.0',
	icon: null,
	web: '/plugins/demo/web/index.js?v=abcd1234',
	page: { title: 'Demo app', icon: 'eye' },
	styles: [],
	skills: [],
	enabled: true,
	settings: [],
	config: {},
	instanceSettings: [],
	instanceSettingValues: {},
	backend: false,
	...over
});

let host: HTMLElement;
let component: Record<string, unknown>;

function render() {
	host = document.createElement('div');
	document.body.appendChild(host);
	component = mount(Route, { target: host });
	flushSync();
}

function journey(name: string): HTMLElement | null {
	return host.querySelector(`[data-journey="${name}"]`);
}

beforeEach(() => {
	document.body.innerHTML = '';
	pluginLoader.entries = {};
	state.url = new URL('https://app.test/apps/demo');
	state.params = { id: 'demo', path: '' };
	query.data = undefined;
	query.isPending = true;
	query.isError = false;
	enabled.value = {};
	goto.mockClear();
	loaded.page = undefined;
	loaded.mounted = [];
});

describe('/apps/<id> route', () => {
	it('is loading while the plugin list is in flight', () => {
		render();
		expect(journey('plugin-page-loading')).not.toBeNull();
		unmount(component);
	});

	it('tells the user the plugin is off for them, and offers the switch', () => {
		query.data = [info()];
		query.isPending = false;
		render();
		expect(journey('plugin-page-not-enabled')).not.toBeNull();
		host.querySelector('button')?.click();
		flushSync();
		expect(goto).toHaveBeenCalledWith('/settings/plugins');
		unmount(component);
	});

	it('reports an id no plugin claims', () => {
		query.data = [];
		query.isPending = false;
		render();
		expect(journey('plugin-page-unknown')).not.toBeNull();
		unmount(component);
	});

	it('reports a plugin that declares no page', () => {
		query.data = [info({ page: null })];
		query.isPending = false;
		enabled.value = { demo: true };
		render();
		expect(journey('plugin-page-no-page')).not.toBeNull();
		unmount(component);
	});

	it('surfaces a bundle that failed to load', () => {
		query.data = [info()];
		query.isPending = false;
		enabled.value = { demo: true };
		pluginLoader.entries = {
			'/plugins/demo/web/index.js?v=abcd1234': { info: info(), status: 'failed', error: 'boom' }
		};
		render();
		const failed = journey('plugin-page-failed');
		expect(failed).not.toBeNull();
		expect(host.textContent).toContain('boom');
		unmount(component);
	});

	it('mounts the page with the plugin-relative path and a navigate that goes through SvelteKit', () => {
		const pageComponent = (_anchor: unknown, props: { basePath: string; path: string; navigate(p: string): void }) => {
			loaded.mounted.push({ basePath: props.basePath, path: props.path });
			props.navigate('/pr/12');
		};
		query.data = [info()];
		query.isPending = false;
		enabled.value = { demo: true };
		state.url = new URL('https://app.test/apps/demo/pr/7/files');
		pluginLoader.entries = {
			'/plugins/demo/web/index.js?v=abcd1234': {
				info: info(),
				status: 'ready',
				module: { cctuiApi: 1, page: pageComponent as never }
			}
		};
		render();
		expect(journey('plugin-page')).not.toBeNull();
		expect(loaded.mounted).toEqual([{ basePath: '/apps/demo', path: '/pr/7/files' }]);
		expect(goto).toHaveBeenCalledWith('/apps/demo/pr/12', { noScroll: true, keepFocus: true });
		unmount(component);
	});
});
