// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

const GB = 1024 ** 3;
const machines = {
	data: [
		{
			machine_id: 'm1',
			name: 'ci-runner-02',
			display_name: null,
			hue: 120,
			liveness: 'online',
			connected: true,
			last_seen_at: new Date().toISOString(),
			updated_at: new Date().toISOString(),
			resources: {
				cpu_pct: 82,
				mem_pct: 47,
				mem_used_bytes: 7 * GB,
				mem_total_bytes: 15 * GB,
				disk_pct: 91,
				disk_used_bytes: 424 * GB,
				disk_total_bytes: 466 * GB,
				disk_path: '/home/ci',
				load1: 1.5
			}
		}
	]
};

vi.mock('#lib/queries/index.ts', () => ({
	qk: { machineResources: ['machine-resources'] },
	useMachineResources: () => machines
}));
vi.mock('@tanstack/svelte-query', () => ({ useQueryClient: () => ({ setQueryData: vi.fn() }) }));
vi.mock('#lib/ws.svelte.ts', () => ({ ws: { onMachineResources: () => () => {} } }));
vi.mock('#lib/settings.svelte.ts', () => ({ settings: { monitoredMachines: ['m1'] } }));

import ResourceBattery from './ResourceBattery.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

async function openPanel(): Promise<HTMLElement> {
	comp = mount(ResourceBattery, { target: document.body });
	flushSync();
	const panel = document.querySelector('[popover]') as HTMLElement;
	panel.dispatchEvent(Object.assign(new Event('toggle'), { newState: 'open' }));
	flushSync();
	await Promise.resolve();
	flushSync();
	return panel;
}

describe('the machine popover', () => {
	it('shows CPU, memory and disk as labelled, toned meters with their percent', async () => {
		const panel = await openPanel();
		const stats = [...panel.querySelectorAll('.stat')].map((s) => ({
			text: s.textContent?.replace(/\s+/g, ' ').trim(),
			tone: (s.querySelector('.meter') as HTMLElement).dataset.tone,
			fill: (s.querySelector('.meter .fill') as HTMLElement).style.width
		}));
		expect(stats).toEqual([
			{ text: 'CPU 82% load 1.50', tone: 'warn', fill: '82%' },
			{ text: 'Memory 47% 7.0 GB / 15 GB', tone: 'ok', fill: '47%' },
			{ text: 'Disk 91% 424 GB / 466 GB', tone: 'danger', fill: '91%' }
		]);
		expect(panel.textContent).toContain('/home/ci');
	});

	it('hatches the meters of a machine that went quiet', async () => {
		machines.data[0].updated_at = new Date(Date.now() - 3600_000).toISOString();
		const panel = await openPanel();
		const meters = [...panel.querySelectorAll('.stat .meter')] as HTMLElement[];
		expect(meters.map((m) => m.dataset.tone)).toEqual(['unknown', 'unknown', 'unknown']);
		expect(panel.querySelector('.stat .fill')).toBeNull();
	});
});
