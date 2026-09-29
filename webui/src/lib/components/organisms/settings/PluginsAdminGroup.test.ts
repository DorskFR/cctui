// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { AdminPluginInfo } from '@bindings/AdminPluginInfo';
import type { CatalogPluginInfo } from '@bindings/CatalogPluginInfo';

const query = vi.hoisted(() => ({
	data: undefined as AdminPluginInfo[] | undefined,
	isPending: true,
	isError: false
}));
const catalogQuery = vi.hoisted(() => ({
	data: [] as CatalogPluginInfo[],
	isPending: false,
	isError: false
}));
const endpoints = vi.hoisted(() => ({
	installPluginFromUrl: vi.fn(async () => ({})),
	installPluginUpload: vi.fn(async () => ({})),
	installPluginFromCatalog: vi.fn(async () => ({})),
	setPluginInstanceEnabled: vi.fn(async () => ({})),
	uninstallPlugin: vi.fn(async () => undefined)
}));
const invalidateQueries = vi.hoisted(() => vi.fn(async () => undefined));
vi.mock('$lib/queries', () => ({
	useAdminPlugins: () => query,
	useAdminPluginCatalog: () => catalogQuery,
	endpoints,
	qk: {
		plugins: ['plugins'],
		adminPlugins: ['admin', 'plugins'],
		adminPluginCatalog: ['admin', 'plugins', 'catalog']
	}
}));
vi.mock('@tanstack/svelte-query', () => ({ useQueryClient: () => ({ invalidateQueries }) }));

import PluginsAdminGroup from './PluginsAdminGroup.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
	vi.clearAllMocks();
});

function render(data: AdminPluginInfo[] | undefined, catalog: CatalogPluginInfo[] = []) {
	Object.assign(query, { data, isPending: data === undefined, isError: false });
	Object.assign(catalogQuery, { data: catalog, isPending: false, isError: false });
	comp = mount(PluginsAdminGroup, { target: document.body, props: { isAdmin: true } });
	flushSync();
}

const installed: AdminPluginInfo = {
	id: 'yubisashi',
	name: 'Review',
	description: 'Frames the app.',
	version: '0.5.0',
	source: 'installed',
	enabled: false
};
const fromDir: AdminPluginInfo = { ...installed, id: 'local', name: 'Local', source: 'directory', enabled: true };

const published = (over: Partial<CatalogPluginInfo> = {}): CatalogPluginInfo => ({
	id: 'yubisashi',
	name: 'Review',
	description: 'Point at your running app.',
	version: '0.5.2',
	homepage: 'https://github.com/DorskFR/yubisashi',
	installed_version: null,
	update_available: false,
	...over
});

const tick = () => new Promise((r) => setTimeout(r, 0));

describe('PluginsAdminGroup', () => {
	it('lists every plugin with its source; only installed ones get a toggle and uninstall', () => {
		render([installed, fromDir]);
		const rows = document.querySelectorAll<HTMLElement>('[data-journey="plugin-admin-row"]');
		expect([...rows].map((r) => r.dataset.plugin)).toEqual(['yubisashi', 'local']);
		expect(rows[0].textContent).toContain('0.5.0');
		expect(rows[0].textContent).toContain('installed');
		expect(rows[1].textContent).toContain('from directory');
		expect(document.querySelectorAll('[data-journey="plugin-admin-switch"]')).toHaveLength(1);
		expect(document.querySelectorAll('[data-journey="plugin-admin-uninstall"]')).toHaveLength(1);
	});

	it('toggles the instance flag and refreshes both plugin lists', async () => {
		render([installed]);
		document.querySelector<HTMLElement>('[data-journey="plugin-admin-switch"]')?.click();
		flushSync();
		await tick();
		expect(endpoints.setPluginInstanceEnabled).toHaveBeenCalledWith('yubisashi', true);
		expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['admin', 'plugins'] });
		expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['plugins'] });
	});

	it('installs from a URL and clears the field', async () => {
		render([]);
		expect(document.querySelector('[data-journey="plugins-admin-empty"]')).not.toBeNull();
		const input = document.querySelector<HTMLInputElement>('[data-journey="plugin-admin-url"]');
		const button = document.querySelector<HTMLButtonElement>('[data-journey="plugin-admin-install"]');
		if (!input || !button) throw new Error('install controls missing');
		expect(button.disabled).toBe(true);
		input.value = ' https://example.com/p.tgz ';
		input.dispatchEvent(new Event('input'));
		flushSync();
		expect(button.disabled).toBe(false);
		button.click();
		await tick();
		expect(endpoints.installPluginFromUrl).toHaveBeenCalledWith('https://example.com/p.tgz');
		flushSync();
		expect(input.value).toBe('');
	});

	it('offers Install, Update or Installed from the catalog annotations', () => {
		render([], [
			published(),
			published({ id: 'stale', name: 'Stale', installed_version: '0.4.0', update_available: true }),
			published({ id: 'current', name: 'Current', installed_version: '0.5.2', homepage: null })
		]);
		const rows = document.querySelectorAll<HTMLElement>('[data-journey="plugin-catalog-row"]');
		expect([...rows].map((r) => r.dataset.plugin)).toEqual(['yubisashi', 'stale', 'current']);
		expect(rows[0].textContent).toContain('Point at your running app.');
		expect(rows[0].textContent).toContain('0.5.2');
		const button = (id: string) =>
			document.querySelector<HTMLButtonElement>(`[data-journey="plugin-catalog-install"][data-plugin="${id}"]`);
		expect(button('yubisashi')?.textContent?.trim()).toBe('Install');
		expect(button('yubisashi')?.disabled).toBe(false);
		expect(button('stale')?.textContent?.trim()).toBe('Update to 0.5.2');
		expect(button('stale')?.disabled).toBe(false);
		expect(button('current')?.textContent?.trim()).toBe('Installed');
		expect(button('current')?.disabled).toBe(true);
		const home = rows[0].querySelector<HTMLAnchorElement>('[data-journey="plugin-catalog-homepage"]');
		expect(home?.getAttribute('href')).toBe('https://github.com/DorskFR/yubisashi');
		expect(rows[2].querySelector('[data-journey="plugin-catalog-homepage"]')).toBeNull();
	});

	it('installs a catalog entry by id and refreshes the catalog too', async () => {
		render([], [published()]);
		document.querySelector<HTMLElement>('[data-journey="plugin-catalog-install"]')?.click();
		flushSync();
		await tick();
		expect(endpoints.installPluginFromCatalog).toHaveBeenCalledWith('yubisashi');
		expect(endpoints.installPluginFromUrl).not.toHaveBeenCalled();
		expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['admin', 'plugins', 'catalog'] });
		expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['admin', 'plugins'] });
		expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['plugins'] });
	});

	it('says so when the catalog is empty', () => {
		render([]);
		expect(document.querySelector('[data-journey="plugins-catalog-empty"]')).not.toBeNull();
		expect(document.querySelector('[data-journey="plugin-catalog-row"]')).toBeNull();
	});

	it('uninstalls after confirmation only', async () => {
		render([installed]);
		const confirm = vi.fn(() => false);
		vi.stubGlobal('confirm', confirm);
		document.querySelector<HTMLElement>('[data-journey="plugin-admin-uninstall"]')?.click();
		await tick();
		expect(endpoints.uninstallPlugin).not.toHaveBeenCalled();
		confirm.mockReturnValue(true);
		document.querySelector<HTMLElement>('[data-journey="plugin-admin-uninstall"]')?.click();
		await tick();
		expect(endpoints.uninstallPlugin).toHaveBeenCalledWith('yubisashi');
		vi.unstubAllGlobals();
	});
});
