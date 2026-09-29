// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { PluginInfo } from '$lib/plugins/types';

const query = vi.hoisted(() => ({ data: undefined as PluginInfo[] | undefined, isPending: true, isError: false }));
const adminQuery = vi.hoisted(() => ({ data: [] as unknown[], isPending: false, isError: false }));
const adminGate = vi.hoisted(() => vi.fn<(enabled: () => boolean) => typeof adminQuery>());
vi.mock('$lib/queries', () => ({
	usePlugins: () => query,
	useAdminPlugins: (enabled: () => boolean) => adminGate(enabled),
	useAdminPluginCatalog: (enabled: () => boolean) => adminGate(enabled),
	endpoints: {
		installPluginFromUrl: vi.fn(async () => ({})),
		installPluginUpload: vi.fn(async () => ({})),
		installPluginFromCatalog: vi.fn(async () => ({})),
		setPluginInstanceEnabled: vi.fn(async () => ({})),
		uninstallPlugin: vi.fn(async () => undefined)
	},
	qk: { plugins: ['plugins'], adminPlugins: ['admin', 'plugins'], adminPluginCatalog: ['admin', 'plugins', 'catalog'] }
}));
vi.mock('@tanstack/svelte-query', () => ({
	useQueryClient: () => ({ invalidateQueries: vi.fn(async () => undefined) })
}));

import PluginsSection from './PluginsSection.svelte';
import { settings } from '$lib/settings.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
	vi.clearAllMocks();
});

function render(data: PluginInfo[] | undefined, over: Partial<typeof query> = {}, isAdmin = false) {
	Object.assign(query, { data, isPending: data === undefined, isError: false }, over);
	adminGate.mockReturnValue(adminQuery);
	comp = mount(PluginsSection, { target: document.body, props: { isAdmin } });
	flushSync();
}

const plugin = (over: Partial<PluginInfo> = {}): PluginInfo => ({
	id: 'yubisashi',
	name: 'Review',
	description: 'Frames the app.',
	version: '0.3.0',
	icon: 'eye',
	web: '/plugins/yubisashi/web/index.js?v=1',
	skills: ['yubisashi'],
	enabled: false,
	settings: [],
	config: {},
	...over
});

describe('PluginsSection', () => {
	it('explains CCTUI_PLUGINS_DIR when nothing is installed', () => {
		render([]);
		const empty = document.querySelector('[data-journey="plugins-empty"]');
		expect(empty?.textContent).toContain('CCTUI_PLUGINS_DIR');
		expect(empty?.textContent).toContain('on this page');
		expect(document.querySelector('[data-journey="plugin-switch"]')).toBeNull();
	});
	it('renders the admin block above the user list for admins', () => {
		settings.setPluginEnabled('yubisashi', false);
		render([plugin()], {}, true);
		const admin = document.querySelector('[data-journey="plugins-admin"]');
		const userSwitch = document.querySelector('[data-journey="plugin-switch"]');
		expect(admin).not.toBeNull();
		expect(userSwitch).not.toBeNull();
		if (!admin || !userSwitch) throw new Error('unreachable');
		expect(admin.compareDocumentPosition(userSwitch) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
		expect(adminGate).toHaveBeenCalled();
		expect(adminGate.mock.calls.every(([enabled]) => enabled())).toBe(true);
	});
	it('hides the admin block from non-admins and never enables the admin queries', () => {
		settings.setPluginEnabled('yubisashi', false);
		render([plugin()]);
		expect(document.querySelector('[data-journey="plugins-admin"]')).toBeNull();
		expect(document.querySelector('[data-journey="plugin-admin-url"]')).toBeNull();
		expect(adminGate.mock.calls.some(([enabled]) => enabled())).toBe(false);
	});
	it('lists what the server reports with a switch bound to settings', () => {
		settings.setPluginEnabled('yubisashi', false);
		render([plugin(), plugin({ id: 'notes', name: 'Notes', web: null, version: '2.0.0' })]);
		const switches = document.querySelectorAll<HTMLElement>('[data-journey="plugin-switch"]');
		expect([...switches].map((s) => s.dataset.plugin)).toEqual(['yubisashi', 'notes']);
		expect(document.body.textContent).toContain('Frames the app.');
		expect(document.body.textContent).toContain('0.3.0');
		expect(document.body.textContent).toContain('2.0.0');
		expect(switches[0].getAttribute('aria-checked')).toBe('false');
		switches[0].click();
		flushSync();
		expect(settings.pluginsEnabled.yubisashi).toBe(true);
		settings.setPluginEnabled('yubisashi', false);
	});
	it('renders the settings form only once the user enabled the plugin', () => {
		settings.setPluginEnabled('yubisashi', false);
		render([plugin({ settings: [{ key: 'host', label: 'Bind address', env: 'YUBI_HOST', type: 'string' }] })]);
		expect(document.querySelector('[data-journey="plugin-config"]')).toBeNull();
		document.querySelector<HTMLElement>('[data-journey="plugin-switch"]')?.click();
		flushSync();
		expect(document.querySelectorAll('[data-journey="plugin-config"]')).toHaveLength(1);
		settings.setPluginEnabled('yubisashi', false);
	});
	it('renders a field per declared setting, with its env var, saving into plugins.config', () => {
		settings.setPluginEnabled('yubisashi', true);
		settings.setPluginConfig('yubisashi', 'host', '');
		render([
			plugin({
				settings: [
					{ key: 'host', label: 'Bind address', env: 'YUBI_HOST', type: 'string' },
					{ key: 'advertise', label: 'Advertised host name', env: 'YUBI_ADVERTISE', type: 'string' }
				]
			}),
			plugin({ id: 'plain', name: 'Plain' })
		]);
		expect(document.querySelectorAll('[data-journey="plugin-config"]')).toHaveLength(1);
		const fields = document.querySelectorAll<HTMLInputElement>('[data-journey="plugin-config-field"]');
		expect([...fields].map((f) => f.dataset.key)).toEqual(['host', 'advertise']);
		expect(document.body.textContent).toContain('YUBI_HOST');
		expect(document.body.textContent).toContain('YUBI_ADVERTISE');
		expect(document.body.textContent).toContain('private to you');
		fields[0].value = ' 0.0.0.0 ';
		fields[0].dispatchEvent(new Event('blur'));
		flushSync();
		expect(settings.pluginConfig('yubisashi')).toEqual({ host: '0.0.0.0' });
		settings.setPluginConfig('yubisashi', 'host', '');
		settings.setPluginEnabled('yubisashi', false);
	});
	it('shows a failure state when the list cannot load', () => {
		render(undefined, { isPending: false, isError: true });
		expect(document.body.textContent).toContain('could not be loaded');
	});
});
