// @vitest-environment happy-dom
import { flushSync } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { drafts, LAST_SPAWN_NAME, SPAWN_SLOT, spawnSlotKey } from '$lib/drafts';
import { FOLLOWUP_RELATION } from '$lib/followup';
import { attachmentStore } from '$lib/attachmentStore';
import { SpawnForm, type SpawnFormOptions } from './spawnForm.svelte';
import { saveDraft } from './spawnSubmit';
import { NO_ACCOUNT } from './options';

const machineList = [{ id: 'm-uuid-1', name: 'box', display_name: 'box', kind: 'persistent', hue: null }];
let dispatcherList: string[] = [];
const spawn = vi.fn();
const updateDraft = vi.fn();
const setDraftAttachments = vi.fn();

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
		useSessionActions: () => ({
			spawn,
			updateDraft,
			setDraftAttachments,
			discardDraft: async () => {}
		}),
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
const settle = () => new Promise((r) => setTimeout(r, 0));
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
	setDraftAttachments.mockReset();
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
		vi.useFakeTimers();
		const sf = open();
		sf.form.working_dir = '/w';
		sf.form.prompt = 'typed';
		sf.envRows = [{ key: 'SECRET', value: 'never-on-disk' }];
		flushSync();
		vi.advanceTimersByTime(300);
		vi.useRealTimers();
		const saved = JSON.parse(drafts.get(SLOT));
		expect(saved.prompt).toBe('typed');
		expect(saved.envRows).toEqual([{ key: 'SECRET', value: '' }]);
		expect(drafts.get(SPAWN_SLOT)).toBe(SLOT);
	});
});

describe('SpawnForm draft persistence', () => {
	afterEach(() => vi.useRealTimers());

	it('writes the slot once typing pauses, not on every keystroke', () => {
		vi.useFakeTimers();
		const sf = open();
		sf.form.working_dir = '/w';
		flushSync();
		vi.advanceTimersByTime(300);
		const set = vi.spyOn(drafts, 'set');
		for (const ch of 'hello') {
			sf.form.prompt += ch;
			flushSync();
			vi.advanceTimersByTime(50);
		}
		expect(set.mock.calls.filter(([k]) => k === SLOT)).toHaveLength(0);
		vi.advanceTimersByTime(300);
		expect(set.mock.calls.filter(([k]) => k === SLOT)).toHaveLength(1);
		expect(JSON.parse(drafts.get(SLOT)).prompt).toBe('hello');
	});

	it('flushes a pending write when the form closes', () => {
		vi.useFakeTimers();
		const sf = open();
		sf.form.working_dir = '/w';
		sf.form.prompt = 'last words';
		flushSync();
		stop?.();
		stop = undefined;
		expect(JSON.parse(drafts.get(SLOT)).prompt).toBe('last words');
	});
});

describe('SpawnForm large paste', () => {
	const paste = (text: string) => {
		let prevented = false;
		const e = {
			preventDefault: () => (prevented = true),
			clipboardData: { items: [], files: [], getData: () => text }
		} as unknown as ClipboardEvent;
		return { e, prevented: () => prevented };
	};

	it('stages a long paste as a paste-N.txt attachment and keeps it out of the prompt', async () => {
		const sf = open();
		await settle();
		const text = Array.from({ length: 3000 }, (_, i) => `line ${i}`).join('\n');
		const p = paste(text);
		sf.att.onPaste(p.e);
		expect(p.prevented()).toBe(true);
		await vi.waitFor(() => expect(sf.files.map((f) => f.name)).toEqual(['paste-1.txt']));
		expect(await sf.files[0].text()).toBe(text);
		expect(sf.form.prompt).not.toContain('line 0');
		expect(sf.form.prompt).toContain('[paste-1.txt]');
	});

	it('leaves a short paste to the field', () => {
		const sf = open();
		const p = paste('short');
		sf.att.onPaste(p.e);
		expect(p.prevented()).toBe(false);
		expect(sf.files).toEqual([]);
	});

	it('clears an oversized restored prompt in one action', () => {
		drafts.set(SLOT, JSON.stringify({ machine_id: 'm-uuid-1', working_dir: '/w', prompt: 'x\n'.repeat(3000) }));
		drafts.set(SPAWN_SLOT, SLOT);
		const sf = open();
		sf.clearForm();
		expect(sf.form.prompt).toBe('');
		expect(drafts.get(SLOT)).toBe('');
	});
});

describe('SpawnForm attachment names', () => {
	const shot = (name: string) => new File(['x'], name, { type: 'text/plain' });

	it('marks attached files at the prompt caret and sends their names in place', async () => {
		const sf = open();
		await settle();
		sf.form.working_dir = '/w';
		sf.form.prompt = 'look here: and there';
		const el = document.createElement('textarea');
		el.value = sf.form.prompt;
		el.setSelectionRange(10, 10);
		sf.promptEl = el;
		sf.att.add([shot('a.txt'), shot('b.txt')]);
		await vi.waitFor(() => expect(sf.files).toHaveLength(2));
		expect(sf.form.prompt).toBe('look here: [a.txt] [b.txt] and there');
		expect(sf.buildSpawnBody().prompt).toBe('look here: [a.txt] [b.txt] and there');
	});

	it("drops a removed file's name from the prompt", async () => {
		const sf = open();
		await settle();
		sf.att.add([shot('a.txt'), shot('b.txt')]);
		await vi.waitFor(() => expect(sf.files).toHaveLength(2));
		sf.att.remove('a.txt');
		expect(sf.form.prompt).toBe('[b.txt]');
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

describe('SpawnForm draft attachments', () => {
	const settle = () => new Promise((r) => setTimeout(r, 0));

	it('ships the files with the draft and re-sends them only when the set changes', async () => {
		spawn.mockResolvedValue({ command_id: 'draft-1', status: 'draft' });
		updateDraft.mockResolvedValue({ command_id: 'draft-1', status: 'draft' });
		setDraftAttachments.mockResolvedValue([]);
		const sf = open();
		sf.form.working_dir = '/w';
		sf.form.prompt = 'see [shot.png]';
		const shot = new File(['png'], 'shot.png');
		const notes = new File(['md'], 'notes.md');
		sf.files = [shot, notes];

		await saveDraft(sf);
		expect(spawn).toHaveBeenCalledTimes(1);
		expect(spawn.mock.calls[0][0]).toMatchObject({
			save_draft: true,
			attachment_names: ['shot.png', 'notes.md']
		});
		expect(spawn.mock.calls[0][1]).toEqual([shot, notes]);
		expect(setDraftAttachments).not.toHaveBeenCalled();
	});

	it('autosave saves the text only and leaves the files in the browser', async () => {
		spawn.mockResolvedValue({ command_id: 'draft-1', status: 'draft' });
		updateDraft.mockResolvedValue({ command_id: 'draft-1', status: 'draft' });
		const sf = open();
		sf.form.working_dir = '/w';
		sf.form.prompt = 'see [shot.png]';
		sf.files = [new File(['png'], 'shot.png')];
		flushSync();

		expect(await sf.flushDraft()).toBe(true);
		expect(spawn).toHaveBeenCalledTimes(1);
		expect(spawn.mock.calls[0][1]).toEqual([]);

		expect(await sf.flushDraft()).toBe(true);
		expect(updateDraft).toHaveBeenCalledTimes(1);
		expect(setDraftAttachments).not.toHaveBeenCalled();
	});

	it('uploads the files once the user saves a draft autosave created', async () => {
		updateDraft.mockResolvedValue({ command_id: 'draft-1', status: 'draft' });
		setDraftAttachments.mockResolvedValue([]);
		await attachmentStore.clearAll();
		const sf = open({ prefill: { machine_id: 'm-uuid-1', working_dir: '/w', draft_id: 'draft-1' } });
		sf.form.prompt = 'p';
		const shot = new File(['png'], 'saved.png');
		sf.files = [shot];
		flushSync();

		expect(await sf.flushDraft()).toBe(true);
		expect(setDraftAttachments).not.toHaveBeenCalled();

		await saveDraft(sf);
		expect(setDraftAttachments).toHaveBeenCalledTimes(1);
		expect(setDraftAttachments.mock.calls[0]).toEqual(['draft-1', [shot]]);
	});

	it('keeps a file attached while the stored set was still restoring', async () => {
		const restored = new File(['old'], 'restored.png');
		await attachmentStore.set(SLOT, [restored]);
		localStorage.setItem('cctui_spawn_slot', SLOT);
		drafts.set(SLOT, JSON.stringify({ machine_id: 'm-uuid-1', working_dir: '/w', prompt: '[restored.png] [pasted.txt]', attachmentNames: ['restored.png', 'pasted.txt'] }));
		const sf = open();
		const pasted = new File(['new'], 'pasted.txt');
		sf.files = [pasted];
		await settle();
		expect(sf.files.map((f) => f.name)).toEqual(['restored.png', 'pasted.txt']);
		expect(sf.form.prompt).toBe('[restored.png] [pasted.txt]');
	});
});
