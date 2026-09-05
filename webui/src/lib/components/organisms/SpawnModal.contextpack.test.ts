import { mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import SpawnModal from './SpawnModal.svelte';

const machineList = [
	{ id: 'm-uuid-1', name: 'box', display_name: 'box', kind: 'persistent', hue: null }
];
const dispatch = vi.fn();

vi.mock('$lib/queries', () => {
	const q = <T>(data: T) => ({ data, isLoading: false, isError: false });
	return {
		useAllMachines: () => q(machineList),
		useDispatchers: () => q([{ id: 'kube', name: 'kube' }]),
		useRecentDirs: () => q([]),
		useAccounts: () => q([]),
		useAccountPools: () => q([]),
		useLabels: () => q({ labels: [] }),
		useSessionActions: () => ({ dispatch, discardDraft: async () => {} }),
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

let component: ReturnType<typeof mount> | undefined;

beforeEach(() => {
	localStorage.clear();
	dispatch.mockReset().mockResolvedValue({ dispatcher: 'kube', handle: 'h-1' });
});
afterEach(async () => {
	if (component) await unmount(component);
	component = undefined;
	document.body.replaceChildren();
});

const tick = (ms = 30) => new Promise((r) => setTimeout(r, ms));

async function openDispatch(prefill: Record<string, string> | null = null) {
	component = mount(SpawnModal, {
		target: document.body,
		props: { onclose: () => {}, onspawned: () => {}, prefill, autosaveDelay: 10_000 }
	});
	await tick(60);
	const tabs = [...document.querySelectorAll<HTMLButtonElement>('[role="tab"]')];
	tabs[1].click();
	await tick(60);
}

function field(id: string): HTMLInputElement | HTMLTextAreaElement {
	const el = document.querySelector<HTMLInputElement | HTMLTextAreaElement>(`#${id}`);
	if (!el) throw new Error(`#${id} not found`);
	return el;
}
async function typeInto(id: string, value: string) {
	const el = field(id);
	el.value = value;
	el.dispatchEvent(new Event('input', { bubbles: true }));
	await tick();
}
async function expandAdvanced() {
	const btn = [...document.querySelectorAll<HTMLButtonElement>('button')].find(
		(b) => b.textContent?.trim() === 'Advanced'
	);
	if (!btn) throw new Error('advanced toggle not found');
	btn.click();
	await tick();
}
async function addEnvRow(key: string, value: string) {
	const add = [...document.querySelectorAll<HTMLButtonElement>('button')].find((b) =>
		b.textContent?.includes('env var')
	);
	if (!add) throw new Error('add env var button not found');
	add.click();
	await tick();
	const keyEl = document.querySelector<HTMLInputElement>('input[aria-label="Secret name"]');
	const valEl = document.querySelector<HTMLInputElement>('input[aria-label="Secret value"]');
	if (!keyEl || !valEl) throw new Error('env row inputs not found');
	for (const [el, v] of [
		[keyEl, key],
		[valEl, value]
	] as const) {
		el.value = v;
		el.dispatchEvent(new Event('input', { bubbles: true }));
	}
	await tick();
}
async function submitDispatch() {
	const btn = [...document.querySelectorAll<HTMLButtonElement>('button')].find(
		(b) => b.textContent?.trim() === 'Dispatch'
	);
	if (!btn) throw new Error('dispatch button not found');
	btn.click();
	await tick(60);
}
const sentEnv = () =>
	(dispatch.mock.calls[0][0].payload as { env?: Record<string, string> }).env ?? {};

describe('SpawnModal context pack', () => {
	it('is dispatch-only: the machine tab has no pack fields', async () => {
		component = mount(SpawnModal, {
			target: document.body,
			props: { onclose: () => {}, onspawned: () => {}, prefill: null, autosaveDelay: 10_000 }
		});
		await tick(60);
		expect(document.querySelector('#sp-pack-url')).toBeNull();
	});

	it('sends the pack fields as CONTEXT_PACK_* env', async () => {
		await openDispatch();
		await typeInto('sp-prompt-d', 'do the thing');
		await typeInto('sp-pack-url', 'https://github.com/org/pack');
		await expandAdvanced();
		await typeInto('sp-pack-ref', 'v1.2.3');
		await typeInto('sp-pack-subdir', 'packs/cctui');
		await typeInto('sp-pack-token', 'vault:secret/pack#token');
		await submitDispatch();
		expect(dispatch).toHaveBeenCalledTimes(1);
		expect(sentEnv()).toEqual({
			CONTEXT_PACK_URL: 'https://github.com/org/pack',
			CONTEXT_PACK_REF: 'v1.2.3',
			CONTEXT_PACK_SUBDIR: 'packs/cctui',
			CONTEXT_PACK_TOKEN: 'vault:secret/pack#token'
		});
	});

	it('adds nothing when the pack fields are empty', async () => {
		await openDispatch();
		await typeInto('sp-prompt-d', 'do the thing');
		await submitDispatch();
		expect(dispatch).toHaveBeenCalledTimes(1);
		expect((dispatch.mock.calls[0][0].payload as { env?: unknown }).env).toBeUndefined();
	});

	it('lets the explicit field win over a duplicate raw env row', async () => {
		await openDispatch();
		await typeInto('sp-prompt-d', 'do the thing');
		await addEnvRow('CONTEXT_PACK_URL', 'https://github.com/org/typo');
		await typeInto('sp-pack-url', 'https://github.com/org/pack');
		await submitDispatch();
		expect(sentEnv()).toEqual({ CONTEXT_PACK_URL: 'https://github.com/org/pack' });
	});

	it('keeps unrelated raw env rows alongside the pack', async () => {
		await openDispatch();
		await typeInto('sp-prompt-d', 'do the thing');
		await addEnvRow('OTHER', 'v');
		await typeInto('sp-pack-url', 'https://github.com/org/pack');
		await submitDispatch();
		expect(sentEnv()).toEqual({
			OTHER: 'v',
			CONTEXT_PACK_URL: 'https://github.com/org/pack'
		});
	});

	it('never writes the pack token to the local draft slot', async () => {
		await openDispatch();
		await typeInto('sp-pack-url', 'https://github.com/org/pack');
		await expandAdvanced();
		await typeInto('sp-pack-token', 'ghp_secret');
		await tick(60);
		expect(localStorage.getItem('cctui_spawn_slot')).toBeTruthy();
		expect(JSON.stringify(localStorage)).not.toContain('ghp_secret');
		const slot = JSON.parse(localStorage.getItem(localStorage.getItem('cctui_spawn_slot')!)!);
		expect(slot.context_pack_url).toBe('https://github.com/org/pack');
		expect(slot.context_pack_token).toBeUndefined();
	});
});
