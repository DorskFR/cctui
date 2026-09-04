import { mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import SpawnModal from './SpawnModal.svelte';

const machineList = [
	{ id: 'm-uuid-1', name: 'box', display_name: 'box', kind: 'persistent', hue: null }
];

vi.mock('$lib/queries', () => {
	const q = <T>(data: T) => ({ data, isLoading: false, isError: false });
	return {
		useAllMachines: () => q(machineList),
		useDispatchers: () => q([]),
		useRecentDirs: () => q([]),
		useAccounts: () => q([]),
		useAccountPools: () => q([]),
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
		lastDirFor: () => null,
		lastEntryFor: () => null,
		recallSpawn: () => null,
		rememberSpawn: () => {}
	}
}));

vi.mock('$lib/ws.svelte', () => ({ ws: { sessions: [] } }));

const DRAFT = 'cctui_spawn_draft';
const SECRET = 'sk-super-secret-token-123';

let component: ReturnType<typeof mount> | undefined;

beforeEach(() => {
	localStorage.clear();
});
afterEach(async () => {
	if (component) await unmount(component);
	component = undefined;
	document.body.replaceChildren();
});

const tick = (ms = 50) => new Promise((r) => setTimeout(r, ms));

async function open() {
	component = mount(SpawnModal, {
		target: document.body,
		props: { onclose: () => {}, onspawned: () => {} }
	});
	await tick(100);
}

function inputByLabel(label: string): HTMLInputElement {
	const el = document.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`);
	if (!el) throw new Error(`input "${label}" not found`);
	return el;
}

async function typeInto(input: HTMLInputElement, value: string) {
	input.focus();
	input.value = value;
	input.dispatchEvent(new Event('input', { bubbles: true }));
	await tick();
}

const slot = () => localStorage.getItem(localStorage.getItem('cctui_spawn_slot') ?? DRAFT);
function draftEnvRows(): { key: string; value: string }[] {
	const raw = slot();
	return raw ? (JSON.parse(raw).envRows ?? []) : [];
}

describe('SpawnModal env draft persistence', () => {
	it('never writes a typed env value to localStorage', async () => {
		await open();
		const add = [...document.querySelectorAll<HTMLButtonElement>('button')].find((b) =>
			/env/i.test(b.textContent ?? '')
		);
		if (!add) throw new Error('add env button not found');
		add.click();
		await tick();

		await typeInto(inputByLabel('Secret name'), 'API_KEY');
		await typeInto(inputByLabel('Secret value'), SECRET);

		expect(inputByLabel('Secret value').value).toBe(SECRET);
		expect(slot()).not.toContain(SECRET);
		expect(draftEnvRows()).toEqual([{ key: 'API_KEY', value: '' }]);
	});

	it('strips values from an existing draft on load and rewrites it', async () => {
		localStorage.setItem(
			DRAFT,
			JSON.stringify({ name: 'x', envRows: [{ key: 'TOKEN', value: SECRET }] })
		);
		await open();

		expect(slot()).not.toContain(SECRET);
		expect(draftEnvRows()).toEqual([{ key: 'TOKEN', value: '' }]);
		expect(inputByLabel('Secret name').value).toBe('TOKEN');
		expect(inputByLabel('Secret value').value).toBe('');
	});
});
