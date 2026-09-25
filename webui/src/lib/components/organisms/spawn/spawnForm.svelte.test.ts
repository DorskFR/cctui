// @vitest-environment happy-dom
import { flushSync } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { drafts, LAST_SPAWN_NAME, SPAWN_SLOT, spawnSlotKey } from '$lib/drafts';
import { FOLLOWUP_RELATION } from '$lib/followup';
import { SpawnForm, type SpawnFormOptions } from './spawnForm.svelte';
import { NO_ACCOUNT } from './options';

const machineList = [{ id: 'm-uuid-1', name: 'box', display_name: 'box', kind: 'persistent', hue: null }];
let dispatcherList: string[] = [];
const spawn = vi.fn();
const updateDraft = vi.fn();

vi.mock('$lib/queries', () => {
	const q = <T>(data: T) => ({ data, isLoading: false, isError: false });
	return {
		useAllMachines: () => q(machineList),
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
		useSessionActions: () => ({ spawn, updateDraft, discardDraft: async () => {} }),
		endpoints: { machineDirs: async () => [], sessions: async () => ({ sessions: [] }) }
	};
});

vi.mock('$lib/settings.svelte', () => ({
	settings: {
		state: { display: { archiveShortcut: true } },
		lastDirFor: () => null,
		lastEntryFor: () => null,
		recallSpawn: () => null,
		rememberSpawn: () => {}
	}
}));

vi.mock('$lib/ws.svelte', () => ({ ws: { sessions: [] } }));

const SLOT = spawnSlotKey('m-uuid-1', '/w');
let stop: (() => void) | undefined;

function open(opts: Partial<SpawnFormOptions> = {}): SpawnForm {
	let sf!: SpawnForm;
	stop = $effect.root(() => {
		sf = new SpawnForm({
			onclose: () => {},
			onspawned: () => {},
			autosaveDelay: () => 10_000,
			...opts
		});
	});
	flushSync();
	return sf;
}

beforeEach(() => {
	localStorage.clear();
	dispatcherList = [];
	spawn.mockReset();
	updateDraft.mockReset();
});
afterEach(() => {
	stop?.();
	stop = undefined;
});

describe('SpawnForm draft restore', () => {
	it('resumes the slot last in progress, re-proposing env keys without values', () => {
		drafts.set(
			SLOT,
			JSON.stringify({
				machine_id: 'm-uuid-1',
				working_dir: '/w',
				prompt: 'resume me',
				name: 'run-3',
				draftId: 'd-1',
				envRows: [{ key: 'TOKEN', value: 'leaked?' }],
				attachmentNames: []
			})
		);
		drafts.set(SPAWN_SLOT, SLOT);
		const sf = open();
		expect(sf.form.prompt).toBe('resume me');
		expect(sf.form.name).toBe('run-3');
		expect(sf.form.working_dir).toBe('/w');
		expect(sf.draftId).toBe('d-1');
		expect(sf.envRows).toEqual([{ key: 'TOKEN', value: '' }]);
		expect(sf.isDirty()).toBe(true);
	});

	it('proposes the next session name on a fresh open', () => {
		drafts.set(LAST_SPAWN_NAME, 'run-7');
		const sf = open();
		expect(sf.form.name).toBe('run-8');
		expect(sf.form.machine_id).toBe('m-uuid-1');
		expect(sf.isDirty()).toBe(true);
	});

	it('keeps the persisted slot current with the form', () => {
		const sf = open();
		sf.form.working_dir = '/w';
		sf.form.prompt = 'typed';
		sf.envRows = [{ key: 'SECRET', value: 'never-on-disk' }];
		flushSync();
		const saved = JSON.parse(drafts.get(SLOT));
		expect(saved.prompt).toBe('typed');
		expect(saved.envRows).toEqual([{ key: 'SECRET', value: '' }]);
		expect(drafts.get(SPAWN_SLOT)).toBe(SLOT);
	});
});

describe('SpawnForm prefill', () => {
	it('seeds the form, env keys, labels and draft id from the prefill', () => {
		const sf = open({
			prefill: {
				machine_id: 'm-uuid-1',
				working_dir: '/w',
				prompt: 'go',
				draft_id: 'd-9',
				env_keys: 'A,B',
				label_ids: 'l1,l2'
			}
		});
		expect(sf.form.prompt).toBe('go');
		expect(sf.form.working_dir).toBe('/w');
		expect(sf.draftId).toBe('d-9');
		expect(sf.envRows).toEqual([
			{ key: 'A', value: '' },
			{ key: 'B', value: '' }
		]);
		expect(sf.form.labels).toEqual(['l1', 'l2']);
	});

	it('lets non-empty prefill values win over the slot without clearing the rest', () => {
		drafts.set(SLOT, JSON.stringify({ machine_id: 'm-uuid-1', working_dir: '/w', prompt: 'old', name: 'kept' }));
		const sf = open({ prefill: { machine_id: 'm-uuid-1', working_dir: '/w', prompt: 'new', name: '' } });
		expect(sf.form.prompt).toBe('new');
		expect(sf.form.name).toBe('kept');
	});

	it('marks a follow-up and its archive choice', () => {
		const sf = open({
			prefill: {
				machine_id: 'm-uuid-1',
				working_dir: '/w',
				relation: FOLLOWUP_RELATION,
				parent_session_id: 's-parent',
				archive_source: '1'
			}
		});
		expect(sf.followupParent).toBe('s-parent');
		expect(sf.archiveSource).toBe(true);
		expect(sf.buildSpawnBody().parent_session_id).toBe('s-parent');
	});
});

describe('SpawnForm validation', () => {
	it('needs a machine and a cwd to spawn, and a prompt to autosave', () => {
		const sf = open();
		expect(sf.spawnValid).toBe(false);
		expect(sf.valid).toBe(false);
		sf.form.working_dir = '/w';
		expect(sf.spawnValid).toBe(true);
		expect(sf.valid).toBe(true);
		expect(sf.draftValid).toBe(true);
		expect(sf.autosaveReady).toBe(false);
		sf.form.prompt = 'do it';
		expect(sf.autosaveReady).toBe(true);
	});

	it('rejects malformed env keys for launch but not for a draft', () => {
		const sf = open();
		sf.form.working_dir = '/w';
		sf.envRows = [{ key: 'bad-key', value: 'x' }];
		expect(sf.badEnvKeys).toHaveLength(1);
		expect(sf.secretsValid).toBe(false);
		expect(sf.valid).toBe(false);
		expect(sf.draftValid).toBe(true);
	});

	it('validates the dispatch target on its own fields', () => {
		dispatcherList = ['k8s-a'];
		const sf = open();
		flushSync();
		expect(sf.canDispatch).toBe(true);
		expect(sf.form.dispatcher).toBe('k8s-a');
		sf.setTarget('dispatch');
		expect(sf.target).toBe('dispatch');
		expect(sf.valid).toBe(false);
		expect(sf.draftValid).toBe(false);
		sf.form.prompt = 'run';
		expect(sf.valid).toBe(true);
		sf.setTarget('anything-else');
		expect(sf.target).toBe('machine');
	});
});

describe('SpawnForm spawn body', () => {
	it('maps the account choice onto the request', () => {
		const sf = open();
		sf.form.working_dir = '/w/';
		sf.form.prompt = ' hi ';
		sf.form.labels = ['l1'];
		let body = sf.buildSpawnBody();
		expect(body.working_dir).toBe('/w');
		expect(body.prompt).toBe('hi');
		expect(body.auto_account).toBe(true);
		expect(body.no_account).toBe(false);
		expect(body.label_ids).toEqual(['l1']);
		expect(body.save_draft).toBe(false);
		sf.form.account = NO_ACCOUNT;
		body = sf.buildSpawnBody();
		expect(body.no_account).toBe(true);
		expect(body.auto_account).toBe(false);
		expect(body.account).toBeNull();
	});

	it('mirrors env keys and attachment names into the draft body', () => {
		const sf = open();
		sf.envRows = [{ key: ' K1 ', value: 'v' }, { key: '', value: '' }];
		sf.files = [new File(['x'], 'notes.md')];
		const body = sf.draftBody();
		expect(body.env).toEqual({});
		expect(body.env_keys).toEqual(['K1']);
		expect(body.attachment_names).toEqual(['notes.md']);
	});
});
