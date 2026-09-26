import { drafts, LAST_SPAWN_NAME } from '$lib/drafts';
import {
	machineMemoryKey,
	dispatchMemoryKey,
	applyMemory,
	dirPrefill,
	labelPrefill,
	memoryFieldsOf,
	MACHINE_MEMORY_FIELDS,
	DISPATCH_MEMORY_FIELDS,
	type MemoryField,
	type MemoryPatch,
	type SpawnMemoryEntry
} from '$lib/spawnMemory';
import { settings } from '$lib/settings.svelte';
import type { Form } from './types';

/**
 * Spawn memory: the config last submitted for a (machine, cwd), recalled
 * whenever the machine or cwd changes. An explicit edit in the open form (or a
 * restored draft / prefill) wins over the memory, which wins over the blank
 * seed. The cursors record what each recall already wrote so a later run can
 * tell an edit from its own writes. `seeded` is the modal's baseline snapshot.
 */
export class SpawnRecall {
	private readonly initialFields: Record<MemoryField, string>;
	private readonly initialDir: string;
	private readonly initialLabels: string[];
	private readonly suppressFirst: boolean;
	private readonly prefillDir: boolean;
	private readonly loadedDir: boolean;
	private memApplied: MemoryPatch = {};
	private labelsApplied: string[] | null = null;
	private dirComboApplied: string | null = null;
	private dirApplied: string | null = null;
	private memKeyApplied: string | null = null;
	private dispatchKeyApplied: string | null = null;

	constructor(
		seeded: Form,
		opts: { prefill: boolean; prefillDir: boolean; loadedDraft: boolean }
	) {
		this.initialFields = memoryFieldsOf(seeded);
		this.initialDir = seeded.working_dir;
		this.initialLabels = [...seeded.labels];
		this.suppressFirst = opts.prefill || opts.loadedDraft;
		this.prefillDir = opts.prefillDir;
		this.loadedDir = opts.loadedDraft && !!seeded.working_dir;
	}

	/** Picking a machine pre-fills its most recent working dir. Keyed on
	 * (machine, remembered dir) so the fill re-attempts once the settings blob
	 * hydrates. A draft/prefill only suppresses the fill when it carries a dir. */
	machineDir(form: Form, recentDirs: string[]): string | null {
		const id = form.machine_id;
		if (!id) return null;
		const last = settings.lastDirFor(id) ?? recentDirs[0] ?? null;
		const combo = machineMemoryKey(id, last ?? '');
		if (combo === this.dirComboApplied) return null;
		const first = this.dirComboApplied === null;
		this.dirComboApplied = combo;
		if (first && (this.prefillDir || this.loadedDir)) return null;
		const next = dirPrefill(form.working_dir, last, this.dirApplied ?? this.initialDir);
		if (next === null) return null;
		this.dirApplied = next;
		return next;
	}

	machine(form: Form): { patch: Partial<Form>; labels: string[] | null } | null {
		const key = form.machine_id ? machineMemoryKey(form.machine_id, form.working_dir) : null;
		if (!key || key === this.memKeyApplied) return null;
		const first = this.memKeyApplied === null;
		this.memKeyApplied = key;
		if (first && this.suppressFirst) return null;
		const entry = settings.recallSpawn(key) ?? settings.lastEntryFor(form.machine_id);
		if (!entry) return null;
		const patch = this.apply(MACHINE_MEMORY_FIELDS, form, entry);
		const labels = labelPrefill(form.labels, this.initialLabels, this.labelsApplied, entry);
		if (labels) this.labelsApplied = labels;
		return { patch, labels };
	}

	dispatch(form: Form): Partial<Form> | null {
		const key = form.dispatcher ? dispatchMemoryKey(form.dispatcher, form.repo) : null;
		if (!key || key === this.dispatchKeyApplied) return null;
		const first = this.dispatchKeyApplied === null;
		this.dispatchKeyApplied = key;
		if (first && this.suppressFirst) return null;
		const entry = settings.recallSpawn(key);
		if (!entry) return null;
		return this.apply(DISPATCH_MEMORY_FIELDS, form, entry);
	}

	private apply(fields: readonly MemoryField[], form: Form, entry: SpawnMemoryEntry): Partial<Form> {
		const patch = applyMemory(
			fields,
			form,
			this.initialFields,
			this.memApplied,
			entry,
			drafts.get(LAST_SPAWN_NAME)
		);
		this.memApplied = { ...this.memApplied, ...patch };
		return patch as Partial<Form>;
	}
}
