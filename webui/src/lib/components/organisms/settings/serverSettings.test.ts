// @vitest-environment happy-dom
import { flushSync, mount, unmount, tick } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { SpawnDefaultsInfo } from '@bindings/SpawnDefaultsInfo';
import type { UpstreamHostsInfo } from '@bindings/UpstreamHostsInfo';

const api = vi.hoisted(() => ({
	spawnDefaults: vi.fn(),
	setSpawnDefaults: vi.fn(),
	upstreamHosts: vi.fn(),
	setUpstreamHosts: vi.fn()
}));
vi.mock('$lib/queries', () => ({ endpoints: api }));
vi.mock('$lib/toast.svelte', () => ({ toasts: { ok: vi.fn(), error: vi.fn() } }));

import SpawnLimitsGroup from './SpawnLimitsGroup.svelte';
import UpstreamHostsGroup from './UpstreamHostsGroup.svelte';
import { parseSpawnDraft } from './serverSettings.logic';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.replaceChildren();
	vi.clearAllMocks();
});

const settle = async () => {
	for (let i = 0; i < 5; i++) await tick();
	flushSync();
};
const button = (text: string) =>
	[...document.querySelectorAll('button')].find((b) => b.textContent?.trim() === text) as
		| HTMLButtonElement
		| undefined;
const input = (label: string) =>
	document.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`) as HTMLInputElement;
const type = (el: HTMLInputElement, value: string) => {
	el.value = value;
	el.dispatchEvent(new Event('input', { bubbles: true }));
	flushSync();
};

const spawnInfo = (over: Partial<SpawnDefaultsInfo> = {}): SpawnDefaultsInfo => ({
	effective: { max_children: 16, max_depth: 2, max_tree_budget_usd: 400 },
	sources: { max_children: 'default', max_depth: 'env', max_tree_budget_usd: 'default' },
	settings: { max_children: null, max_depth: null, max_tree_budget_usd: null },
	env: { max_children: null, max_depth: 2, max_tree_budget_usd: null },
	defaults: { max_children: 16, max_depth: 3, max_tree_budget_usd: 400 },
	...over
});

describe('parseSpawnDraft', () => {
	it('maps empty to null and rejects invalid values', () => {
		expect(parseSpawnDraft({ max_children: '', max_depth: ' 3 ', max_tree_budget_usd: '0' })).toEqual({
			max_children: null,
			max_depth: 3,
			max_tree_budget_usd: 0
		});
		for (const bad of [
			{ max_children: '0', max_depth: '', max_tree_budget_usd: '' },
			{ max_children: '2.5', max_depth: '', max_tree_budget_usd: '' },
			{ max_children: '', max_depth: 'x', max_tree_budget_usd: '' },
			{ max_children: '', max_depth: '', max_tree_budget_usd: '-1' }
		]) {
			expect(parseSpawnDraft(bad)).toBeNull();
		}
	});
});

describe('CctuiAgent limits', () => {
	it('shows effective value with its source and saves a new value', async () => {
		api.spawnDefaults.mockResolvedValue(spawnInfo());
		comp = mount(SpawnLimitsGroup, { target: document.body });
		await settle();
		expect(document.body.textContent).toContain('Effective: 2 (from env)');
		expect(document.body.textContent).toContain('Effective: 16 (built-in default)');
		expect(button('Save')?.disabled).toBe(true);

		api.setSpawnDefaults.mockResolvedValue(
			spawnInfo({
				effective: { max_children: 4, max_depth: 2, max_tree_budget_usd: 400 },
				sources: { max_children: 'settings', max_depth: 'env', max_tree_budget_usd: 'default' },
				settings: { max_children: 4, max_depth: null, max_tree_budget_usd: null }
			})
		);
		type(input('Max live children'), '4');
		button('Save')?.click();
		await settle();
		expect(api.setSpawnDefaults).toHaveBeenCalledWith({
			max_children: 4,
			max_depth: null,
			max_tree_budget_usd: null
		});
		expect(document.body.textContent).toContain('Effective: 4 (saved in Settings)');

		api.setSpawnDefaults.mockResolvedValue(spawnInfo());
		button('Reset')?.click();
		await settle();
		expect(api.setSpawnDefaults).toHaveBeenLastCalledWith({
			max_children: null,
			max_depth: null,
			max_tree_budget_usd: null
		});
	});

	it('blocks saving an invalid value', async () => {
		api.spawnDefaults.mockResolvedValue(spawnInfo());
		comp = mount(SpawnLimitsGroup, { target: document.body });
		await settle();
		type(input('Max depth'), '0');
		expect(button('Save')?.disabled).toBe(true);
		expect(document.body.textContent).toContain('positive whole numbers');
	});
});

describe('allowed upstream hosts', () => {
	const hostsInfo = (over: Partial<UpstreamHostsInfo> = {}): UpstreamHostsInfo => ({
		hosts: [],
		source: 'default',
		env: ['ollama.llm.svc'],
		managed: ['litellm.llm.svc:4000'],
		...over
	});

	it('shows env and managed hosts as fixed; saved entries add, remove and reset', async () => {
		api.upstreamHosts.mockResolvedValue(hostsInfo());
		comp = mount(UpstreamHostsGroup, { target: document.body });
		await settle();
		expect(document.body.textContent).toContain('ollama.llm.svc');
		expect(document.body.textContent).toContain('from env');
		expect(document.body.textContent).toContain('litellm.llm.svc:4000');
		expect(document.querySelector('button[aria-label="Remove ollama.llm.svc"]')).toBeNull();
		expect(button('Reset')).toBeUndefined();

		api.setUpstreamHosts.mockResolvedValue(hostsInfo({ hosts: ['10.0.0.5:8080'], source: 'settings' }));
		type(input('host or host:port'), ' 10.0.0.5:8080 ');
		button('Add')?.click();
		await settle();
		expect(api.setUpstreamHosts).toHaveBeenCalledWith(['10.0.0.5:8080']);
		expect(document.body.textContent).toContain('saved in Settings');
		expect(document.body.textContent).toContain('ollama.llm.svc');
		expect(document.querySelector('button[aria-label="Remove ollama.llm.svc"]')).toBeNull();

		api.setUpstreamHosts.mockResolvedValue(hostsInfo({ hosts: [], source: 'settings' }));
		document.querySelector<HTMLButtonElement>('button[aria-label="Remove 10.0.0.5:8080"]')?.click();
		await settle();
		expect(api.setUpstreamHosts).toHaveBeenLastCalledWith([]);

		api.setUpstreamHosts.mockResolvedValue(hostsInfo());
		button('Reset')?.click();
		await settle();
		expect(api.setUpstreamHosts).toHaveBeenLastCalledWith(null);
	});
});
