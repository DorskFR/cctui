// @vitest-environment happy-dom
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { MachineAdapterInfo } from '@bindings/MachineAdapterInfo';
import type { MachineRow } from '@bindings/MachineRow';

type Pending = { resolve: (rows: MachineAdapterInfo[]) => void };
const pending = vi.hoisted(() => new Map<string, Pending[]>());
const machinesQuery = vi.hoisted(() => ({ data: undefined as MachineRow[] | undefined }));
const endpoints = vi.hoisted(() => ({
	machineAdapters: vi.fn(
		(id: string) =>
			new Promise<MachineAdapterInfo[]>((resolve) => {
				pending.set(id, [...(pending.get(id) ?? []), { resolve }]);
			})
	),
	setMachineAdapter: vi.fn(),
	resetMachineAdapter: vi.fn()
}));
vi.mock('$lib/queries', () => ({
	endpoints,
	useAllMachines: () => machinesQuery,
	qk: { machineAdapters: (id: string) => ['machine-adapters', id] }
}));
vi.mock('@tanstack/svelte-query', () => ({
	useQueryClient: () => ({ invalidateQueries: vi.fn(async () => undefined) })
}));
vi.mock('$lib/toast.svelte', () => ({ toasts: { ok: vi.fn(), error: vi.fn() } }));
vi.mock('$lib/harnesses.svelte', () => ({ harnessTable: () => [] }));

import MachineAdaptersGroup from './MachineAdaptersGroup.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	pending.clear();
	vi.clearAllMocks();
	document.body.innerHTML = '';
});

const machine = (id: string) => ({ id, name: id, display_name: null }) as MachineRow;
const adapter = (enabled: boolean): MachineAdapterInfo =>
	({ adapter_id: 'codex', enabled, pinned: true, config: null, binary: null }) as MachineAdapterInfo;
const settle = async () => {
	for (let i = 0; i < 4; i++) await tick();
	flushSync();
};
const switchOf = () => document.querySelector<HTMLButtonElement>('button[role="switch"]');

describe('MachineAdaptersGroup loading', () => {
	it('requests each machine once, whatever order the responses land in', async () => {
		machinesQuery.data = [machine('a'), machine('b'), machine('c')];
		comp = mount(MachineAdaptersGroup, { target: document.body });
		flushSync();
		expect(endpoints.machineAdapters).toHaveBeenCalledTimes(3);
		pending.get('b')![0].resolve([adapter(true)]);
		await settle();
		pending.get('a')![0].resolve([]);
		await settle();
		expect(endpoints.machineAdapters).toHaveBeenCalledTimes(3);
		expect(pending.get('c')).toHaveLength(1);
	});

	it('a late initial response never overrides a toggle saved in the meantime', async () => {
		machinesQuery.data = [machine('a')];
		comp = mount(MachineAdaptersGroup, { target: document.body });
		flushSync();
		const [first] = pending.get('a')!;
		first.resolve([adapter(false)]);
		await settle();
		const sw = switchOf();
		expect(sw, 'switch rendered').not.toBeNull();
		endpoints.setMachineAdapter.mockResolvedValueOnce([adapter(true)]);
		sw!.click();
		await settle();
		expect(endpoints.setMachineAdapter).toHaveBeenCalledWith('a', 'codex', { enabled: true });
		expect(endpoints.machineAdapters).toHaveBeenCalledTimes(1);
		expect(switchOf()!.getAttribute('aria-checked')).toBe('true');
	});
});
