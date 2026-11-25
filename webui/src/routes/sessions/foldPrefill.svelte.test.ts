// @vitest-environment happy-dom
import { flushSync } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { FOLLOWUP_RELATION } from '#lib/followup.ts';
import { SpawnForm } from '#lib/components/organisms/spawn/spawnForm.svelte.ts';
import { foldedPrefill } from './sessions.logic';

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
let dispatcherList: string[] = [];
const spawn = vi.fn();
const updateDraft = vi.fn();
const setDraftAttachments = vi.fn();
const listSessions = vi.fn();
const discardDraft = vi.fn();
const launchDraft = vi.fn();
const scheduleDraftLaunch = vi.fn();

vi.mock('#lib/queries/index.ts', () => {
	const q = <T>(data: T) => ({ data, isLoading: false, isError: false });
	return {
		useMachineResources: () => q(machineList),
		useDispatchers: () => q(dispatcherList),
		useSessions: () => q({ sessions: [] }),
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
		useSessionActions: () => ({
			spawn,
			updateDraft,
			setDraftAttachments,
			discardDraft,
			launchDraft,
			scheduleDraftLaunch
		}),
		endpoints: { machineDirs: async () => [], sessions: () => listSessions() }
	};
});

vi.mock('#lib/settings.svelte.ts', () => ({
	settings: {
		state: { display: { archiveShortcut: true } },
		lastDirFor: () => null,
		lastEntryFor: () => null,
		recallSpawn: () => null,
		rememberSpawn: () => {}
	}
}));

vi.mock('#lib/ws.svelte.ts', async () => {
	const { CommandWaiters } = await import('#lib/ws/commands.ts');
	return { ws: {
		sessions: [],
		onCommandResult: () => () => {},
		onSessionEnded: () => () => {},
		awaitSpawn: (commandId: string, sessionId: string | null, opts: object) =>
			new CommandWaiters().awaitSpawn(commandId, sessionId, {
				...opts, probeIntervalMs: 1, timeoutMs: 100
			})
	} };
});

let stop: (() => void) | undefined;

function mountForm(prefill: Record<string, string> | null): SpawnForm {
	let sf!: SpawnForm;
	stop = $effect.root(() => {
		sf = new SpawnForm({ onclose: () => {}, onspawned: () => {}, autosaveDelay: () => 10_000, prefill });
	});
	flushSync();
	return sf;
}

function unmountForm() {
	stop?.();
	stop = undefined;
}

beforeEach(() => {
	localStorage.clear();
	listSessions.mockReset().mockResolvedValue({ sessions: [] });
});
afterEach(unmountForm);

const target = { machine_id: 'm-uuid-1', working_dir: '/w' };

describe('docked spawn form across a conversation fold', () => {
	it('keeps an edit made after a prefill when a chat opens and closes', () => {
		const prefill = { ...target, prompt: 'from the script', name: 'seeded' };
		const sf = mountForm(prefill);
		sf.form.prompt = 'edited by hand';
		flushSync();
		unmountForm();

		const again = mountForm(foldedPrefill(prefill));
		expect(again.form.prompt).toBe('edited by hand');
		expect(again.form.name).toBe('seeded');
	});

	it('keeps the follow-up link while dropping the seeded fields', () => {
		const prefill = {
			...target,
			prompt: 'continue',
			relation: FOLLOWUP_RELATION,
			parent_session_id: 'parent-1',
			followup_file: '# brief'
		};
		const sf = mountForm(prefill);
		sf.form.prompt = 'continue, but differently';
		flushSync();
		unmountForm();

		const kept = foldedPrefill(prefill);
		expect(kept).toEqual({
			relation: FOLLOWUP_RELATION,
			parent_session_id: 'parent-1',
			followup_file: '# brief'
		});
		const again = mountForm(kept);
		expect(again.form.prompt).toBe('continue, but differently');
		expect(again.followupParent).toBe('parent-1');
	});

	it('drops a prefill with nothing to keep', () => {
		expect(foldedPrefill(null)).toBeNull();
		expect(foldedPrefill({ ...target, prompt: 'x' })).toBeNull();
	});
});
