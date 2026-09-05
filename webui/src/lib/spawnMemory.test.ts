import { describe, expect, it } from 'vitest';
import {
	applyMemory,
	dirPrefill,
	dispatchMemoryKey,
	DISPATCH_MEMORY_FIELDS,
	entryFromForm,
	labelPrefill,
	latestDirFor,
	latestEntryFor,
	latestProfileFor,
	profileUsage,
	recordProfileUse,
	WEEK_MS,
	MACHINE_MEMORY_FIELDS,
	machineMemoryKey,
	memoryFieldsOf,
	putSpawnMemory,
	SPAWN_MEMORY_CAP,
	type SpawnMemoryEntry,
	type SpawnMemoryMap
} from './spawnMemory';

const SEP = '\u001f';

function entry(over: Partial<SpawnMemoryEntry> = {}): SpawnMemoryEntry {
	return {
		adapter_id: 'claude-code',
		model_claude: 'opus',
		model_codex: '',
		model_account: '',
		effort_claude: 'high',
		effort_codex: '',
		account: 'work',
		account_provider: 'anthropic',
		permission_mode: 'auto',
		name: 'toto',
		at: 1,
		...over
	};
}

const fields = (over: Partial<Record<string, string>> = {}) =>
	memoryFieldsOf({
		adapter_id: 'claude-code',
		model_claude: '',
		model_codex: '',
		model_account: '',
		effort_claude: '',
		effort_codex: '',
		account: '',
		permission_mode: '',
		name: '',
		...over
	} as Record<(typeof MACHINE_MEMORY_FIELDS)[number], string>);

describe('key derivation', () => {
	it('keys machine memory by (machine, normalized cwd)', () => {
		expect(machineMemoryKey('m1', '/home/x/proj')).toBe(`m${SEP}m1${SEP}/home/x/proj`);
		// trailing slashes and padding collapse to the same key
		expect(machineMemoryKey('m1', ' /home/x/proj/ ')).toBe(machineMemoryKey('m1', '/home/x/proj'));
		// the filesystem root survives normalization
		expect(machineMemoryKey('m1', '///')).toBe(`m${SEP}m1${SEP}/`);
	});

	it('keeps machine and dispatch key spaces distinct', () => {
		expect(dispatchMemoryKey('d1', 'repo')).toBe(`d${SEP}d1${SEP}repo`);
		expect(dispatchMemoryKey('d1', ' repo ')).toBe(dispatchMemoryKey('d1', 'repo'));
		expect(machineMemoryKey('x', 'y')).not.toBe(dispatchMemoryKey('x', 'y'));
	});

	it('does not collide when ids/dirs contain key-ish characters', () => {
		expect(machineMemoryKey('a', 'b/c')).not.toBe(machineMemoryKey('a/b', 'c'));
	});
});

describe('putSpawnMemory (LRU cap)', () => {
	it('inserts and refreshes without mutating the input map', () => {
		const m0: SpawnMemoryMap = {};
		const m1 = putSpawnMemory(m0, 'k1', entry({ at: 1 }));
		expect(m0).toEqual({});
		expect(m1.k1.at).toBe(1);
		const m2 = putSpawnMemory(m1, 'k1', entry({ at: 2, name: 'later' }));
		expect(m2.k1).toMatchObject({ at: 2, name: 'later' });
		expect(Object.keys(m2)).toHaveLength(1);
	});

	it('evicts the least-recently-written entries beyond the cap', () => {
		let m: SpawnMemoryMap = {};
		for (let i = 1; i <= SPAWN_MEMORY_CAP; i++) m = putSpawnMemory(m, `k${i}`, entry({ at: i }));
		expect(Object.keys(m)).toHaveLength(SPAWN_MEMORY_CAP);
		m = putSpawnMemory(m, 'fresh', entry({ at: 1000 }));
		expect(Object.keys(m)).toHaveLength(SPAWN_MEMORY_CAP);
		expect(m.k1).toBeUndefined();
		expect(m.fresh).toBeDefined();
		expect(m.k2).toBeDefined();
	});

	it('re-writing an old key refreshes its recency', () => {
		let m: SpawnMemoryMap = {};
		for (let i = 1; i <= SPAWN_MEMORY_CAP; i++) m = putSpawnMemory(m, `k${i}`, entry({ at: i }));
		m = putSpawnMemory(m, 'k1', entry({ at: 999 }));
		m = putSpawnMemory(m, 'fresh', entry({ at: 1000 }));
		expect(m.k1).toBeDefined();
		expect(m.k2).toBeUndefined();
	});

	it('honors a custom cap', () => {
		let m: SpawnMemoryMap = {};
		m = putSpawnMemory(m, 'a', entry({ at: 1 }), 2);
		m = putSpawnMemory(m, 'b', entry({ at: 2 }), 2);
		m = putSpawnMemory(m, 'c', entry({ at: 3 }), 2);
		expect(Object.keys(m).sort()).toEqual(['b', 'c']);
	});
});

describe('latestDirFor', () => {
	it("returns the cwd of the machine's most recent entry", () => {
		let m: SpawnMemoryMap = {};
		m = putSpawnMemory(m, machineMemoryKey('m1', '/old'), entry({ at: 1 }));
		m = putSpawnMemory(m, machineMemoryKey('m1', '/new'), entry({ at: 2 }));
		m = putSpawnMemory(m, machineMemoryKey('m2', '/other'), entry({ at: 3 }));
		m = putSpawnMemory(m, dispatchMemoryKey('m1', '/decoy'), entry({ at: 9 }));
		expect(latestDirFor(m, 'm1')).toBe('/new');
		expect(latestDirFor(m, 'm2')).toBe('/other');
		expect(latestDirFor(m, 'nope')).toBeNull();
	});
});

describe('applyMemory (precedence)', () => {
	it('remembered entry wins over the seeded defaults', () => {
		const initial = fields({ model_claude: 'sonnet' });
		const current = { ...initial };
		const patch = applyMemory(MACHINE_MEMORY_FIELDS, current, initial, null, entry());
		expect(patch).toMatchObject({
			model_claude: 'opus',
			effort_claude: 'high',
			account: 'work',
			permission_mode: 'auto',
			name: 'toto'
		});
		expect(patch).not.toHaveProperty('adapter_id'); // already equal — no write
	});

	it('an explicit user edit in the open modal wins over the memory', () => {
		const initial = fields({ model_claude: 'sonnet' });
		const current = { ...initial, model_claude: 'haiku', account: 'personal' };
		const patch = applyMemory(MACHINE_MEMORY_FIELDS, current, initial, null, entry());
		expect(patch).not.toHaveProperty('model_claude');
		expect(patch).not.toHaveProperty('account');
		expect(patch).toMatchObject({ effort_claude: 'high', name: 'toto' });
	});

	it('a previous memory application does not count as a user edit', () => {
		const initial = fields();
		// first recall wrote these
		const lastApplied = { model_claude: 'opus', account: 'work' };
		const current = { ...initial, ...lastApplied };
		const patch = applyMemory(
			MACHINE_MEMORY_FIELDS,
			current,
			initial,
			lastApplied,
			entry({ model_claude: 'sonnet', account: 'personal' })
		);
		expect(patch).toMatchObject({ model_claude: 'sonnet', account: 'personal' });
	});

	it('a user edit made after a memory application survives the next recall', () => {
		const initial = fields();
		const lastApplied = { model_claude: 'opus' };
		const current = { ...initial, model_claude: 'haiku' }; // edited since
		const patch = applyMemory(
			MACHINE_MEMORY_FIELDS,
			current,
			initial,
			lastApplied,
			entry({ model_claude: 'sonnet' })
		);
		expect(patch).not.toHaveProperty('model_claude');
	});

	it('only touches the requested field set (dispatch subset)', () => {
		const initial = fields();
		const patch = applyMemory(DISPATCH_MEMORY_FIELDS, { ...initial }, initial, null, entry());
		expect(patch).toMatchObject({ model_claude: 'opus', effort_claude: 'high', account: 'work' });
		expect(patch).not.toHaveProperty('adapter_id');
		expect(patch).not.toHaveProperty('permission_mode');
	});

	it('bumps the remembered name only when it was the last submitted one', () => {
		const initial = fields();
		const bumped = applyMemory(MACHINE_MEMORY_FIELDS, { ...initial }, initial, null, entry(), 'toto');
		expect(bumped.name).toBe('toto-2');
		const verbatim = applyMemory(
			MACHINE_MEMORY_FIELDS,
			{ ...initial },
			initial,
			null,
			entry(),
			'other'
		);
		expect(verbatim.name).toBe('toto');
	});
});

describe('entryFromForm', () => {
	it('captures the remembered set and trims the name', () => {
		const e = entryFromForm({
			adapter_id: 'codex',
			model_claude: '',
			model_codex: 'gpt-5',
			model_account: '',
			effort_claude: '',
			effort_codex: 'medium',
			account: 'work',
			account_provider: 'openai',
			permission_mode: 'yolo',
			name: '  run-1  ',
			labels: ['l1']
		});
		expect(e).toEqual({
			adapter_id: 'codex',
			model_claude: '',
			model_codex: 'gpt-5',
			model_account: '',
			effort_claude: '',
			effort_codex: 'medium',
			account: 'work',
			account_provider: 'openai',
			permission_mode: 'yolo',
			name: 'run-1',
			labels: ['l1']
		});
		expect(e).not.toHaveProperty('at');
	});
});

describe('dirPrefill', () => {
	it('fills an empty field with the remembered dir', () => {
		expect(dirPrefill('', '/repo/a', '')).toBe('/repo/a');
		expect(dirPrefill('   ', '/repo/a', '')).toBe('/repo/a');
	});

	it('does nothing without a remembered dir', () => {
		expect(dirPrefill('', null, '')).toBeNull();
	});

	it('never clobbers a user-typed value', () => {
		expect(dirPrefill('/typed/by/user', '/repo/a', '')).toBeNull();
	});

	it('replaces its own earlier auto-fill (machine switch)', () => {
		expect(dirPrefill('/repo/old', '/repo/new', '/repo/old')).toBe('/repo/new');
	});

	it('is idempotent once the field holds the remembered dir', () => {
		expect(dirPrefill('/repo/a', '/repo/a', '')).toBeNull();
	});
});

describe('latestEntryFor', () => {
	it('returns the machine\'s most recent entry across dirs', () => {
		let map: SpawnMemoryMap = {};
		map = putSpawnMemory(map, machineMemoryKey('m1', '/a'), entry({ name: 'old', at: 1 }));
		map = putSpawnMemory(map, machineMemoryKey('m1', '/b'), entry({ name: 'new', at: 9 }));
		map = putSpawnMemory(map, machineMemoryKey('m2', '/c'), entry({ name: 'other', at: 99 }));
		expect(latestEntryFor(map, 'm1')?.name).toBe('new');
		expect(latestEntryFor(map, 'nope')).toBeNull();
	});

	it('ignores dispatch entries', () => {
		const map = putSpawnMemory({}, dispatchMemoryKey('m1', 'repo'), entry({ name: 'disp' }));
		expect(latestEntryFor(map, 'm1')).toBeNull();
	});
});

describe('free-text model ids', () => {
	it('round-trip through memory verbatim', () => {
		const remembered = entryFromForm({
			...fields({ model_codex: 'gpt-6-astra', model_claude: 'claude-opus-4-8' }),
			account_provider: ''
		});
		expect(remembered.model_codex).toBe('gpt-6-astra');
		expect(remembered.model_claude).toBe('claude-opus-4-8');
		const patch = applyMemory(MACHINE_MEMORY_FIELDS, fields(), fields(), null, entry(remembered));
		expect(patch.model_codex).toBe('gpt-6-astra');
		expect(patch.model_claude).toBe('claude-opus-4-8');
	});
});

describe('labelPrefill', () => {
	const withLabels = (labels?: string[]) => entry({ labels });

	it('fills an untouched picker from the remembered set', () => {
		expect(labelPrefill([], [], null, withLabels(['a', 'b']))).toEqual(['a', 'b']);
	});

	it('keeps a user edit', () => {
		expect(labelPrefill(['x'], [], null, withLabels(['a']))).toBeNull();
	});

	it('overwrites what a previous recall wrote', () => {
		expect(labelPrefill(['a'], [], ['a'], withLabels(['b']))).toEqual(['b']);
	});

	it('no-ops on an entry with no remembered labels, or when already equal', () => {
		expect(labelPrefill([], [], null, withLabels(undefined))).toBeNull();
		expect(labelPrefill(['a'], ['a'], null, withLabels(['a']))).toBeNull();
	});
});

describe('latestProfileFor', () => {
	it("names the profile of the machine's latest entry", () => {
		let map: SpawnMemoryMap = {};
		map = putSpawnMemory(map, machineMemoryKey('m1', '/a'), entry({ profile_id: 'p-old', at: 1 }));
		map = putSpawnMemory(map, machineMemoryKey('m1', '/b'), entry({ profile_id: 'p-new', at: 9 }));
		expect(latestProfileFor(map, 'm1')).toBe('p-new');
		expect(latestProfileFor(map, 'm2')).toBeNull();
	});

	it('is null when the latest entry predates profiles', () => {
		const map = putSpawnMemory({}, machineMemoryKey('m1', '/a'), entry());
		expect(latestProfileFor(map, 'm1')).toBeNull();
	});
});

describe('profile usage log', () => {
	const now = 1_000_000_000_000;

	it('counts this week, else reports the last use, else nothing', () => {
		expect(profileUsage('', 'p1', now)).toBeNull();
		let raw = recordProfileUse('', 'p1', now - 2 * WEEK_MS);
		expect(profileUsage(raw, 'p1', now)).toEqual({ lastAt: now - 2 * WEEK_MS });
		raw = recordProfileUse(raw, 'p1', now - 1000);
		raw = recordProfileUse(raw, 'p1', now);
		expect(profileUsage(raw, 'p1', now)).toEqual({ week: 2 });
		expect(profileUsage(raw, 'p2', now)).toBeNull();
	});

	it('survives garbage and caps the log', () => {
		expect(profileUsage('{not json', 'p1', now)).toBeNull();
		let raw = '[]';
		for (let i = 0; i < 250; i++) raw = recordProfileUse(raw, 'p1', now - i);
		expect(JSON.parse(raw).p1).toHaveLength(200);
	});
});
