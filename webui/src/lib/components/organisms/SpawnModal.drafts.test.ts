import { mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import SpawnModal from './SpawnModal.svelte';
import { spawnSlotKey } from '$lib/drafts';
import { attachmentStore } from '$lib/attachmentStore';

const machineList = [
	{ id: 'm-uuid-1', name: 'box', display_name: 'box', kind: 'persistent', hue: null }
];
const spawn = vi.fn();
const updateDraft = vi.fn();

vi.mock('$lib/queries', () => {
	const q = <T>(data: T) => ({ data, isLoading: false, isError: false });
	return {
		useAllMachines: () => q(machineList),
		useDispatchers: () => q([]),
		useRecentDirs: () => q([]),
		useAccounts: () => q([]),
		useAccountPools: () => q([]),
		useLabels: () => q({ labels: [] }),
		useProfiles: () => q([]),
		useProfileActions: () => ({
			create: async () => ({ id: 'p-1', name: 'Default' }),
			update: async () => ({}),
			remove: async () => {}
		}),
		useAllAccountsUsage: () => q([]),
		useSessionActions: () => ({ spawn, updateDraft, discardDraft: async () => {} }),
		useCodexModels: () => q(null),
		useMergedCodexModels: () => q(null),
		useGitInfo: () => async () => ({ is_repo: false, is_worktree: false }),
		useMachineDirs: () => q([]),
		endpoints: { machineDirs: async () => [] }
	};
});

vi.mock('$lib/settings.svelte', () => ({
	settings: {
		lastDirFor: () => null,
		lastEntryFor: () => null,
		recallSpawn: () => null,
		rememberSpawn: () => {}
	}
}));

vi.mock('$lib/ws.svelte', () => ({ ws: { sessions: [] } }));

const SLOT = spawnSlotKey('m-uuid-1', '/w');
let component: ReturnType<typeof mount> | undefined;

beforeEach(async () => {
	localStorage.clear();
	await attachmentStore.clearAll();
	spawn.mockReset().mockResolvedValue({ command_id: 'draft-1', status: 'draft', account: null });
	updateDraft.mockReset().mockResolvedValue({ command_id: 'draft-1', status: 'draft', account: null });
});
afterEach(async () => {
	if (component) await unmount(component);
	component = undefined;
	document.body.replaceChildren();
});

const tick = (ms = 50) => new Promise((r) => setTimeout(r, ms));

async function open(prefill: Record<string, string> | null = null) {
	component = mount(SpawnModal, {
		target: document.body,
		props: { onclose: () => {}, onspawned: () => {}, prefill, autosaveDelay: 40 }
	});
	await tick(100);
}

function field(id: string): HTMLInputElement | HTMLTextAreaElement {
	const el = document.querySelector<HTMLInputElement | HTMLTextAreaElement>(`#${id}`);
	if (!el) throw new Error(`#${id} not found`);
	return el;
}
async function typeInto(el: HTMLInputElement | HTMLTextAreaElement, value: string) {
	el.value = value;
	el.dispatchEvent(new Event('input', { bubbles: true }));
	await tick();
}
const slot = () => JSON.parse(localStorage.getItem(SLOT) ?? '{}');

describe('SpawnModal draft editing', () => {
	it('keeps the form value where the draft field is empty', async () => {
		localStorage.setItem(
			SLOT,
			JSON.stringify({ machine_id: 'm-uuid-1', working_dir: '/w', name: 'keep-me', prompt: 'old' })
		);
		await open({
			draft_id: 'draft-9',
			machine_id: 'm-uuid-1',
			working_dir: '/w',
			prompt: 'draft prompt',
			env_keys: 'TOKEN'
		});
		expect(field('sp-name').value).toBe('keep-me');
		expect(field('sp-prompt').value).toBe('draft prompt');
		expect(document.querySelector<HTMLInputElement>('input[aria-label="Secret name"]')?.value).toBe('TOKEN');
		expect(slot().draftId).toBe('draft-9');
	});
});

describe('SpawnModal server autosave', () => {
	it('creates one draft, then updates it in place', async () => {
		localStorage.setItem(SLOT, JSON.stringify({ machine_id: 'm-uuid-1', working_dir: '/w' }));
		localStorage.setItem('cctui_spawn_slot', SLOT);
		await open();
		await typeInto(field('sp-prompt'), 'first');
		await tick(120);
		expect(spawn).toHaveBeenCalledTimes(1);
		expect(spawn.mock.calls[0][0]).toMatchObject({ save_draft: true, prompt: 'first', env: {} });
		expect(slot().draftId).toBe('draft-1');

		await typeInto(field('sp-prompt'), 'first then more');
		await tick(120);
		expect(spawn).toHaveBeenCalledTimes(1);
		expect(updateDraft).toHaveBeenCalledTimes(1);
		expect(updateDraft.mock.calls[0][0]).toBe('draft-1');
		expect(updateDraft.mock.calls[0][1]).toMatchObject({ prompt: 'first then more' });
	});

	it('stays quiet while the prompt is empty', async () => {
		localStorage.setItem(SLOT, JSON.stringify({ machine_id: 'm-uuid-1', working_dir: '/w' }));
		localStorage.setItem('cctui_spawn_slot', SLOT);
		await open();
		await typeInto(field('sp-name'), 'named');
		await tick(120);
		expect(spawn).not.toHaveBeenCalled();
		expect(updateDraft).not.toHaveBeenCalled();
	});
});
