// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

const api = vi.hoisted(() => ({ spawnDefaults: vi.fn(), setSpawnDefaults: vi.fn() }));
vi.mock('#lib/queries/index.ts', () => ({ endpoints: api }));

import SpawnLimitsGroup from './SpawnLimitsGroup.svelte';

const none = { max_children: null, max_depth: null, max_tree_budget_usd: null };
const infoOf = (settings: typeof none) => ({
	settings,
	effective: { max_children: 8, max_depth: 3, max_tree_budget_usd: null },
	env: none,
	defaults: { max_children: 8, max_depth: 3, max_tree_budget_usd: null },
	sources: { max_children: 'default', max_depth: 'default', max_tree_budget_usd: 'default' }
});

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	vi.useRealTimers();
	vi.clearAllMocks();
	document.body.innerHTML = '';
});

describe('SpawnLimitsGroup', () => {
	it('saves a valid edit after a pause and skips an invalid one', async () => {
		vi.useFakeTimers();
		api.spawnDefaults.mockResolvedValue(infoOf(none));
		api.setSpawnDefaults.mockImplementation(async (s: typeof none) => infoOf(s));
		comp = mount(SpawnLimitsGroup, { target: document.body });
		await vi.runAllTimersAsync();
		flushSync();
		const [children] = document.querySelectorAll<HTMLInputElement>('input');
		children.value = 'x';
		children.dispatchEvent(new Event('input', { bubbles: true }));
		flushSync();
		await vi.advanceTimersByTimeAsync(700);
		expect(api.setSpawnDefaults).not.toHaveBeenCalled();

		children.value = '4';
		children.dispatchEvent(new Event('input', { bubbles: true }));
		flushSync();
		await vi.advanceTimersByTimeAsync(700);
		expect(api.setSpawnDefaults).toHaveBeenCalledWith({ max_children: 4, max_depth: null, max_tree_budget_usd: null });
		await vi.advanceTimersByTimeAsync(2000);
		expect(api.setSpawnDefaults).toHaveBeenCalledTimes(1);
	});
});
