import { mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import SpawnModal from './SpawnModal.svelte';

const machineList = [
	{ id: 'm-uuid-1', name: 'box', display_name: 'box', kind: 'persistent', hue: null }
];
let recentDirsData: string[] = [];
let memoryDir: string | null = null;

vi.mock('$lib/queries', () => {
	const q = <T>(data: T) => ({ data, isLoading: false, isError: false });
	return {
		useAllMachines: () => q(machineList),
		useDispatchers: () => q([]),
		useRecentDirs: () => q(recentDirsData),
		useAccounts: () => q([]),
		useLabels: () => q({ labels: [] }),
		useSessionActions: () => ({}),
		useCodexModels: () => q(null),
		useGitInfo: () => async () => ({ is_repo: false, is_worktree: false }),
		useMachineDirs: () => q([]),
		endpoints: { machineDirs: async () => [] }
	};
});

vi.mock('$lib/settings.svelte', () => ({
	settings: {
		lastDirFor: () => memoryDir,
		lastEntryFor: () => null,
		recallSpawn: () => null,
		rememberSpawn: () => {}
	}
}));

vi.mock('$lib/ws.svelte', () => ({ ws: { sessions: [] } }));

let component: ReturnType<typeof mount> | undefined;

beforeEach(() => {
	localStorage.clear();
	recentDirsData = [];
	memoryDir = null;
});
afterEach(async () => {
	if (component) await unmount(component);
	component = undefined;
	document.body.replaceChildren();
});

async function open() {
	component = mount(SpawnModal, {
		target: document.body,
		props: { onclose: () => {}, onspawned: () => {} }
	});
	await new Promise((r) => setTimeout(r, 100));
}

function cwdValue(): string {
	const inputs = [...document.querySelectorAll<HTMLInputElement>('input')];
	return inputs.map((i) => i.value).find((v) => v.startsWith('cwd:')) ?? '<no cwd field>';
}

describe('SpawnModal cwd prefill', () => {
	it('fills the cwd from the server recent dirs when spawn memory is empty', async () => {
		recentDirsData = ['/home/dorsk/Documents/cctui'];
		await open();
		expect(cwdValue()).toBe('cwd:/home/dorsk/Documents/cctui');
	});

	it('prefers the remembered dir over the recent dirs', async () => {
		recentDirsData = ['/srv/other'];
		memoryDir = '/home/dorsk/Documents/cctui';
		await open();
		expect(cwdValue()).toBe('cwd:/home/dorsk/Documents/cctui');
	});

	it('leaves the cwd empty when there is nothing to recall', async () => {
		await open();
		expect(cwdValue()).toBe('cwd:');
	});

	it('still takes a typed dir, and a cleared field, from the user', async () => {
		recentDirsData = ['/srv/other'];
		await open();
		expect(draftDir()).toBe('/srv/other');

		await type('cwd:/typed/by/hand');
		expect(draftDir()).toBe('/typed/by/hand');

		await type('cwd:');
		expect(draftDir()).toBe('');
	});
});

function draftDir(): string {
	const raw = localStorage.getItem('cctui_spawn_draft');
	return raw ? JSON.parse(raw).working_dir : '<no draft>';
}

async function type(value: string) {
	const input = [...document.querySelectorAll<HTMLInputElement>('input')].find((i) =>
		i.value.startsWith('cwd:')
	);
	if (!input) throw new Error('cwd field not found');
	input.focus();
	input.value = value;
	input.dispatchEvent(new Event('input', { bubbles: true }));
	await new Promise((r) => setTimeout(r, 50));
}
