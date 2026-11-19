// @vitest-environment happy-dom
import { mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { LaunchPreset } from '@bindings/LaunchPreset';
import PresetEditorModal from './PresetEditorModal.svelte';

const machineList = [
	{
		machine_id: 'm-uuid-1',
		name: 'box',
		display_name: 'box',
		hue: null,
		liveness: 'online',
		last_seen_at: '2026-01-01T00:00:00Z',
		resources: null,
		updated_at: null
	}
];
const save = vi.fn();
const setFiles = vi.fn();

vi.mock('#lib/queries/index.ts', () => {
	const q = <T>(data: T) => ({ data, isLoading: false, isError: false });
	return {
		useMachineResources: () => q(machineList),
		useVoiceConfig: () => q(undefined),
		useRecentDirs: () => q([]),
		useAccounts: () => q([]),
		useAccountPools: () => q([]),
		useAllAccountsUsage: () => q([]),
		useLabels: () => q({ labels: [] }),
		useContextItems: () => q([]),
		useResolvedContext: () => q({ rules: [], memories: [] }),
		useSessionActions: () => ({
			createLabel: async () => ({}),
			updateLabel: async () => ({}),
			deleteLabel: async () => {}
		}),
		useCodexModels: () => q(null),
		useMergedCodexModels: () => q(null),
		useHarnessModels: () => q(null),
		useHarnesses: () => q(null),
		useMachineAdapters: () => q(null),
		useGitInfo: () => async () => ({ is_repo: false, is_worktree: false }),
		useMachineDirs: () => q([]),
		endpoints: { machineDirs: async () => [] }
	};
});

vi.mock('#lib/queries/presets.ts', () => ({
	usePresetActions: () => ({ save, setFiles, remove: vi.fn(), run: vi.fn() })
}));

vi.mock('#lib/settings.svelte.ts', () => ({
	settings: {
		state: { display: { archiveShortcut: true } },
		lastDirFor: () => null,
		lastEntryFor: () => null,
		recallSpawn: () => null,
		rememberSpawn: () => {}
	}
}));

vi.mock('#lib/ws.svelte.ts', () => ({ ws: { sessions: [] } }));

const stored: LaunchPreset = {
	id: 'p1',
	user_id: 'u1',
	name: 'Morning report',
	prompt: 'Summarise the night',
	machine_id: 'm-uuid-1',
	working_dir: '/w',
	adapter_id: 'claude-code',
	model: null,
	permission_mode: 'auto',
	account_id: null,
	pool_id: null,
	no_account: false,
	dispatcher_id: null,
	profile: null,
	external_sources: false,
	fresh_secs: 900,
	on_stale: 'followup',
	max_batch: 20,
	cron: '0 7 * * *',
	timezone: 'UTC',
	enabled: true,
	overlap: 'skip',
	next_fire_at: null,
	last_fire_at: null,
	then_preset_id: null,
	notify_url: null,
	env_keys: ['FEED_TOKEN'],
	template: { effort: 'high', label_ids: ['l1'], attachment_names: ['brief.md'] },
	created_at: '2026-11-01T00:00:00Z',
	updated_at: '2026-11-01T00:00:00Z'
};

let component: ReturnType<typeof mount> | undefined;
const tick = (ms = 50) => new Promise((r) => setTimeout(r, ms));

async function open(preset: LaunchPreset | null = stored) {
	component = mount(PresetEditorModal, {
		target: document.body,
		props: { preset, schedule: true, title: 'Edit schedule', onclose: () => {} }
	});
	await tick(100);
}

const button = (label: string) =>
	[...document.querySelectorAll('button')].find((b) => b.textContent?.trim() === label);

beforeEach(() => {
	save.mockReset().mockResolvedValue(stored);
	setFiles.mockReset().mockResolvedValue(stored);
});
afterEach(async () => {
	if (component) await unmount(component);
	component = undefined;
	document.body.replaceChildren();
});

describe('the schedule editor', () => {
	it('is the spawn form: prompt, kit, env and stored files, plus the repeat', async () => {
		await open();
		expect(document.querySelector('[data-testid="preset-spawn-fields"]')).not.toBeNull();
		expect(document.querySelector<HTMLTextAreaElement>('#sp-prompt')?.value).toBe('Summarise the night');
		expect(document.querySelector('[data-testid="env-stored-name"]')?.textContent).toBe('FEED_TOKEN');
		expect(document.querySelector('input[aria-label="Secret value"]')).toBeNull();
		expect(document.querySelector('[data-testid="preset-stored-files"]')?.textContent).toContain('brief.md');
	});

	it('saves the template the fields hold, keeping the stored env and files', async () => {
		await open();
		const prompt = document.querySelector<HTMLTextAreaElement>('#sp-prompt');
		if (!prompt) throw new Error('no prompt field');
		prompt.value = 'Summarise the week';
		prompt.dispatchEvent(new Event('input', { bubbles: true }));
		await tick();
		button('Save')?.click();
		await tick();
		expect(save).toHaveBeenCalledOnce();
		const [id, body] = save.mock.calls[0];
		expect(id).toBe('p1');
		expect(body).toMatchObject({
			prompt: 'Summarise the week',
			machine_id: 'm-uuid-1',
			adapter_id: 'claude-code',
			permission_mode: 'auto',
			env_keys: ['FEED_TOKEN'],
			env: {},
			template: { effort: 'high', label_ids: ['l1'] }
		});
		expect(setFiles).not.toHaveBeenCalled();
	});

	it('clears the stored files on request', async () => {
		await open();
		button('Remove files')?.click();
		await tick();
		button('Save')?.click();
		await tick();
		expect(setFiles).toHaveBeenCalledWith('p1', []);
	});
});
