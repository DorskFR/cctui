// Per-(machine, working-dir) spawn memory (CCT-561): the config last submitted
// from the spawn modal, keyed by where it ran, so a new session on a known
// machine+cwd needs zero config clicks. Dispatch spawns get the same treatment
// keyed by (dispatcher, repo). Pure helpers here; the map itself lives in the
// server-persisted settings blob (settings.svelte.ts, `state.spawnMemory`).
import { normalizeDir, nextSessionName } from './drafts';

export interface SpawnMemoryEntry {
	adapter_id: string;
	model_claude: string;
	model_codex: string;
	model_account: string;
	effort_claude: string;
	effort_codex: string;
	account: string;
	account_provider: string;
	permission_mode: string;
	name: string;
	/** Last-write timestamp; drives LRU eviction and "latest dir on machine". */
	at: number;
}

export type SpawnMemoryMap = Record<string, SpawnMemoryEntry>;

export const SPAWN_MEMORY_CAP = 50;

// The ASCII unit separator never appears in machine ids, dirs, dispatcher ids
// or repos, so it makes an unambiguous key separator (a dir may contain ':').
// Not NUL: the settings blob lives in a Postgres JSONB column, which rejects
// \u0000 in strings.
const SEP = '\u001f';

export function machineMemoryKey(machineId: string, workingDir: string): string {
	return `m${SEP}${machineId}${SEP}${normalizeDir(workingDir.trim())}`;
}

export function dispatchMemoryKey(dispatcherId: string, repo: string): string {
	return `d${SEP}${dispatcherId}${SEP}${repo.trim()}`;
}

/** Insert/refresh `key` and evict the least-recently-written entries beyond
 *  `cap`. Pure — returns a new map. */
export function putSpawnMemory(
	map: SpawnMemoryMap,
	key: string,
	entry: SpawnMemoryEntry,
	cap: number = SPAWN_MEMORY_CAP
): SpawnMemoryMap {
	const next: SpawnMemoryMap = { ...map, [key]: entry };
	const keys = Object.keys(next);
	if (keys.length > cap) {
		keys.sort((a, b) => next[a].at - next[b].at);
		for (const k of keys.slice(0, keys.length - cap)) delete next[k];
	}
	return next;
}

/** The working dir of the machine's most recently written entry, so picking a
 *  machine can pre-fill the cwd (which in turn keys the full memory recall). */
export function latestDirFor(map: SpawnMemoryMap, machineId: string): string | null {
	const prefix = `m${SEP}${machineId}${SEP}`;
	let best: { at: number; dir: string } | null = null;
	for (const [k, e] of Object.entries(map)) {
		if (!k.startsWith(prefix)) continue;
		if (!best || e.at > best.at) best = { at: e.at, dir: k.slice(prefix.length) };
	}
	return best?.dir ?? null;
}

/** Whether the cwd field should be filled with `last` (the machine's
 *  remembered dir), given what it currently holds and what the modal itself
 *  auto-applied so far (`autoApplied`). Returns the dir to write, or null to
 *  leave the field alone. A user-typed value — anything non-empty the modal
 *  didn't write — always wins; an empty field is fair game, since an empty cwd
 *  can't be spawned anyway. */
export function dirPrefill(current: string, last: string | null, autoApplied: string): string | null {
	if (!last || current === last) return null;
	if (current.trim() !== '' && current !== autoApplied) return null;
	return last;
}

// The form fields a remembered entry drives, per target. `account_provider` is
// deliberately absent: the form recomputes it from the selected account.
export const MACHINE_MEMORY_FIELDS = [
	'adapter_id',
	'model_claude',
	'model_codex',
	'model_account',
	'effort_claude',
	'effort_codex',
	'account',
	'permission_mode',
	'name'
] as const;
// Dispatch runs a claude worker: only the claude-family knobs apply.
export const DISPATCH_MEMORY_FIELDS = [
	'model_claude',
	'model_account',
	'effort_claude',
	'account',
	'name'
] as const;

export type MemoryField = (typeof MACHINE_MEMORY_FIELDS)[number];
export type MemoryPatch = Partial<Record<MemoryField, string>>;
type MemoryFieldValues = Readonly<Record<MemoryField, string>>;

/** Compute the prefill patch for a recalled entry, honoring the precedence
 *  "explicit user edit in the open modal > remembered entry > seeded defaults":
 *  a field is only overwritten while it still holds what the modal seeded
 *  (`initial`) or what a previous memory application wrote (`lastApplied`) —
 *  anything the user typed since stays. A remembered name equal to the globally
 *  last-submitted one gets its numeric suffix bumped (serial-spawn convention). */
export function applyMemory(
	fields: readonly MemoryField[],
	current: MemoryFieldValues,
	initial: MemoryFieldValues,
	lastApplied: MemoryPatch | null,
	entry: SpawnMemoryEntry,
	lastGlobalName = ''
): MemoryPatch {
	const patch: MemoryPatch = {};
	for (const f of fields) {
		const reference = lastApplied?.[f] ?? initial[f];
		if (current[f] !== reference) continue;
		let v = entry[f];
		if (f === 'name' && v && v === lastGlobalName) v = nextSessionName(v);
		if (v !== current[f]) patch[f] = v;
	}
	return patch;
}

/** Snapshot the memory-driven fields of a form-shaped object. */
export function memoryFieldsOf(form: MemoryFieldValues): Record<MemoryField, string> {
	const out = {} as Record<MemoryField, string>;
	for (const f of MACHINE_MEMORY_FIELDS) out[f] = form[f];
	return out;
}

/** Build the entry to remember from the submitted form (timestamp added by the
 *  settings store at write time). */
export function entryFromForm(
	form: MemoryFieldValues & { account_provider: string }
): Omit<SpawnMemoryEntry, 'at'> {
	return {
		adapter_id: form.adapter_id,
		model_claude: form.model_claude,
		model_codex: form.model_codex,
		model_account: form.model_account,
		effort_claude: form.effort_claude,
		effort_codex: form.effort_codex,
		account: form.account,
		account_provider: form.account_provider,
		permission_mode: form.permission_mode,
		name: form.name.trim()
	};
}
