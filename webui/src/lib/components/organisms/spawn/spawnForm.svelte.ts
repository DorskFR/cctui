import { imageAttachments } from '$lib/imageAttachments.svelte';
import { errMessage } from '$lib/api';
import type { SpawnRequest } from '@bindings/SpawnRequest';
import type { SessionProfile } from '@bindings/SessionProfile';
import {
	useAllMachines,
	useDispatchers,
	useSessionActions,
	useRecentDirs,
	useAccounts,
	useAccountPools,
	useAllAccountsUsage,
	useLabels,
	useProfiles,
	useProfileActions,
	endpoints
} from '$lib/queries';
import { toasts } from '$lib/toast.svelte';
import { submitChordLabel } from '$lib/platform';
import {
	drafts,
	promptHistory,
	SPAWN_SLOT,
	spawnSlotKey,
	currentSpawnSlot,
	type SpawnSlotPayload,
	LAST_MACHINE,
	FOLLOWUP_ARCHIVE_SOURCE,
	normalizeDir
} from '$lib/drafts';
import { recordProfileUse, PROFILE_USES } from '$lib/spawnMemory';
import { attachFiles, removeFileByName, fileCapError } from '$lib/attachments';
import { attachmentStore, dropMissingTokens } from '$lib/attachmentStore';
import { BRIEF_FILE_NAME, FOLLOWUP_RELATION } from '$lib/followup';
import { settings } from '$lib/settings.svelte';
import { m } from '$lib/paraglide/messages';
import type { EnvRow, Form, SpawnPrefill, Target } from './types';
import { accountBacksAdapter, providerForAdapter, NO_ACCOUNT, poolName } from './options';
import {
	applySpec,
	initialProfile,
	specFromForm,
	specOf,
	uniqueProfileName,
	type ProfileSpecForm
} from './profiles';
import { blank, seedForm } from './spawnSeed';
import { SpawnRecall } from './spawnRecall';
import { buildSpawnBody, draftBody, envMap } from './spawnBody';
import { autosave, dispatchToK8s, saveDraft, spawnOnMachine } from './spawnSubmit';

const ENV_KEY_RE = /^[A-Z_][A-Z0-9_]*$/;

export interface SpawnFormOptions {
	onclose: () => void;
	onspawned: () => void;
	prefill?: SpawnPrefill | null;
	autosaveDelay?: () => number;
	docked?: () => boolean;
}

/**
 * State, derivations and server calls of the spawn form. One instance per
 * mounted SpawnModal; must be constructed during component init (it opens
 * queries and effects).
 */
export class SpawnForm {
	readonly images = imageAttachments();
	readonly onclose: () => void;
	readonly onspawned: () => void;
	private readonly autosaveDelay: () => number;
	private readonly docked: () => boolean;

	private readonly machines = useAllMachines(() => true);
	private readonly dispatchers = useDispatchers(() => true);
	private readonly accounts = useAccounts(() => true);
	private readonly pools = useAccountPools(() => true);
	private readonly usageQuery = useAllAccountsUsage(() => true);
	private readonly labelsQuery = useLabels();
	private readonly profilesQuery = useProfiles();
	private readonly profileActions = useProfileActions();
	readonly actions = useSessionActions();
	readonly labelApi = {
		attachLabel: (sessionId: string, labelId: string) =>
			this.actions.attachLabel(sessionId, labelId),
		listSessions: () => endpoints.sessions(false)
	};

	target = $state<Target>('machine');
	form = $state<Form>({ ...blank });
	private readonly dirsQuery = useRecentDirs(() => this.form.machine_id);
	draftId = $state<string | null>(null);
	// Kept out of `form` (persisted to localStorage drafts) so secret values
	// never reach disk; only env keys go into the draft. Files live in
	// IndexedDB (attachmentStore), keyed like the draft.
	envRows = $state<EnvRow[]>([]);
	files = $state<File[]>([]);
	private filesRestored = $state(false);
	archiveSource = $state(false);
	busy = $state(false);
	selectedProfileId = $state<string | null>(null);
	oneOff = $state<ProfileSpecForm | null>(null);
	usageRaw = $state(drafts.get(PROFILE_USES));
	spawnFailure = $state<string | null>(null);
	pendingDispatchId = $state<string | null>(null);

	// Local autosave slot, one per (machine, cwd): a prefill names its target's
	// slot, a plain open resumes the slot last in progress. The slot's server
	// mirror is the draft row `draftId`, created on the first autosave.
	private slotKey: string;
	private readonly loadKey: string;
	private readonly recall: SpawnRecall;
	private profileMachineApplied: string | null = null;
	private seededDefault = false;
	private autosaveTimer: ReturnType<typeof setTimeout> | null = null;
	autosaving = false;
	private autosaveSnapshot: string | null = null;
	readonly followupParent: string | null;
	private readonly followupFile: string;

	machineList = $derived(this.machines.data ?? []);
	dispatcherIds = $derived(this.dispatchers.data ?? []);
	canDispatch = $derived(this.dispatcherIds.length > 0);
	targetOptions = $derived([
		{ value: 'machine', label: m.spawn_tab_machine() },
		{ value: 'dispatch', label: m.spawn_tab_dispatch() }
	]);
	recentDirs = $derived([...new Set((this.dirsQuery.data ?? []).map(normalizeDir))]);
	allAccounts = $derived(this.accounts.data ?? []);
	allPools = $derived(this.pools.data ?? []);
	allUsage = $derived(this.usageQuery.data ?? []);
	allLabels = $derived(this.labelsQuery.data?.labels ?? []);

	// The selected profile's kit (or its one-off adjustment) is written over
	// the form at submit time: profile → one-off → explicit prompt / where.
	profiles = $derived(this.profilesQuery.data ?? []);
	selectedProfile = $derived(this.profiles.find((p) => p.id === this.selectedProfileId) ?? null);
	profileSpec = $derived(this.oneOff ?? (this.selectedProfile ? specOf(this.selectedProfile) : null));
	effectiveForm = $derived(
		this.target === 'machine' && this.profileSpec
			? applySpec(this.form, this.profileSpec, this.allAccounts, this.allPools)
			: this.form
	);

	selectedAccount = $derived(
		this.effectiveForm.account &&
			this.effectiveForm.account !== NO_ACCOUNT &&
			!poolName(this.effectiveForm.account)
			? this.allAccounts.find((a) => a.name === this.effectiveForm.account)
			: undefined
	);
	spawnProvider = $derived(
		providerForAdapter(this.selectedAccount, this.effectiveForm.adapter_id)?.provider
	);
	harnessValid = $derived(accountBacksAdapter(this.selectedAccount, this.effectiveForm.adapter_id));
	dispatchProvider = $derived(
		providerForAdapter(this.selectedAccount, this.form.dispatch_adapter || 'claude-code')?.provider
	);

	badEnvKeys = $derived(this.envRows.filter((r) => r.key.trim() && !ENV_KEY_RE.test(r.key.trim())));
	fileError = $derived(fileCapError(this.files));
	secretsValid = $derived(
		this.badEnvKeys.length === 0 && !this.fileError && this.images.pending.length === 0
	);
	spawnValid = $derived(!!this.form.machine_id && !!this.form.working_dir.trim() && this.harnessValid);
	dispatchValid = $derived(
		!!this.form.dispatcher && (!!this.form.prompt.trim() || !!this.form.prompt_file.trim())
	);
	valid = $derived(
		(this.target === 'machine' ? this.spawnValid : this.dispatchValid) && this.secretsValid
	);
	autosaveReady = $derived(this.target === 'machine' && this.spawnValid && !!this.form.prompt.trim());
	// Drafts are a machine-spawn concept: valid whenever the spawn form is;
	// secrets needn't be valid yet (entered at launch).
	draftValid = $derived(
		this.target === 'machine' && this.spawnValid && this.images.pending.length === 0
	);
	spawnLabel = $derived(
		`${this.target !== 'machine' ? m.spawn_action_dispatch() : m.spawn_action_spawn()} (${submitChordLabel()})`
	);
	noMachines = $derived(this.target === 'machine' && this.machineList.length === 0);
	disabledReason = $derived(this.noMachines ? m.spawn_no_machines_hint() : undefined);

	constructor(opts: SpawnFormOptions) {
		const prefill = opts.prefill ?? null;
		this.onclose = opts.onclose;
		this.onspawned = opts.onspawned;
		this.autosaveDelay = opts.autosaveDelay ?? (() => 2000);
		this.docked = opts.docked ?? (() => false);
		this.slotKey =
			prefill?.machine_id && prefill.working_dir
				? spawnSlotKey(prefill.machine_id, prefill.working_dir)
				: currentSpawnSlot();
		this.loadKey = this.slotKey;
		const seed = seedForm(prefill, this.slotKey);
		this.form = seed.form;
		this.draftId = seed.draftId;
		this.envRows = seed.envRows;
		this.recall = new SpawnRecall(seed.form, {
			prefill: !!prefill,
			prefillDir: !!prefill?.working_dir,
			loadedDraft: seed.loadedDraft
		});
		this.followupParent =
			prefill?.relation === FOLLOWUP_RELATION ? (prefill.parent_session_id ?? null) : null;
		this.followupFile = this.followupParent ? (prefill?.followup_file ?? '') : '';
		this.archiveSource =
			prefill?.archive_source === '1' || drafts.get(FOLLOWUP_ARCHIVE_SOURCE) === '1';
		this.effects();
	}

	private effects() {
		$effect(() => () => this.images.reset());
		$effect(() => {
			const list = this.machineList;
			if (this.form.machine_id || !list.length) return;
			const last = drafts.get(LAST_MACHINE);
			this.form.machine_id = last && list.some((m) => m.id === last) ? last : list[0].id;
		});
		$effect(() => {
			const next = this.recall.machineDir(this.form, this.recentDirs);
			if (next !== null) this.form.working_dir = next;
		});
		$effect(() => {
			const recalled = this.recall.machine(this.form);
			if (!recalled) return;
			Object.assign(this.form, recalled.patch);
			if (recalled.labels) this.form.labels = recalled.labels;
		});
		$effect(() => {
			const patch = this.recall.dispatch(this.form);
			if (patch) Object.assign(this.form, patch);
		});
		$effect(() => {
			if (this.form.dispatcher || !this.dispatcherIds.length) return;
			this.form.dispatcher = this.dispatcherIds[0];
		});
		// Each machine opens on the profile it last spawned from.
		$effect(() => {
			const profiles = this.profiles;
			if (!profiles.length) return;
			const machine = this.form.machine_id;
			const stillThere =
				!!this.selectedProfileId && profiles.some((p) => p.id === this.selectedProfileId);
			if (machine === this.profileMachineApplied && stillThere) return;
			this.profileMachineApplied = machine;
			const last = settings.lastEntryFor(machine)?.profile_id ?? null;
			this.selectedProfileId = initialProfile(profiles, last)?.id ?? null;
			this.oneOff = null;
		});
		// First open with no profile yet: seed "Default" from the spawn memory.
		$effect(() => {
			if (this.seededDefault || this.profilesQuery.data === undefined || this.profiles.length) return;
			if (!this.form.machine_id || this.accounts.data === undefined || this.pools.data === undefined) {
				return;
			}
			this.seededDefault = true;
			void this.profileActions
				.create({
					name: m.spawn_profile_default_name(),
					...specFromForm(this.form, this.allAccounts, this.allPools)
				})
				.catch(() => {});
		});
		$effect(() => this.persistSlot());
		$effect(() => this.restoreFiles());
		$effect(() => {
			const snapshot = JSON.stringify({
				form: this.form,
				keys: this.envRows.map((r) => r.key),
				names: this.files.map((f) => f.name)
			});
			if (snapshot === this.autosaveSnapshot) return;
			const first = this.autosaveSnapshot === null;
			this.autosaveSnapshot = snapshot;
			if (first) return;
			if (this.autosaveTimer) clearTimeout(this.autosaveTimer);
			this.autosaveTimer = setTimeout(() => void autosave(this), this.autosaveDelay());
		});
		$effect(() => () => this.cancelAutosave());
	}

	private persistSlot() {
		const key = this.form.machine_id
			? spawnSlotKey(this.form.machine_id, this.form.working_dir)
			: this.slotKey;
		if (key !== this.slotKey) {
			drafts.clear(this.slotKey);
			if (this.filesRestored) void attachmentStore.clear(this.slotKey);
			this.slotKey = key;
		}
		const envKeys = this.envRows.map((r) => ({ key: r.key, value: '' }));
		const { context_pack_token: _packToken, ...persisted } = this.form;
		const payload: SpawnSlotPayload = {
			...persisted,
			envRows: envKeys,
			draftId: this.draftId,
			attachmentNames: this.files.map((f) => f.name)
		};
		drafts.set(this.slotKey, JSON.stringify(payload));
		drafts.set(SPAWN_SLOT, this.slotKey);
		if (this.filesRestored) void attachmentStore.set(this.slotKey, [...this.files]);
	}

	private restoreFiles() {
		let live = true;
		(async () => {
			const restored = await attachmentStore.get(this.loadKey);
			if (!live) return;
			this.files = restored.files;
			if (this.followupFile && !this.files.some((f) => f.name === BRIEF_FILE_NAME)) {
				this.files = [
					...this.files,
					new File([this.followupFile], BRIEF_FILE_NAME, { type: 'text/markdown' })
				];
			}
			const { text, dropped } = dropMissingTokens(this.form.prompt, restored.missing);
			if (dropped) {
				this.form.prompt = text;
				toasts.info(m.attachments_missing_dropped({ count: dropped }));
			}
			this.filesRestored = true;
			if (this.loadKey !== this.slotKey) void attachmentStore.clear(this.loadKey);
		})();
		return () => {
			live = false;
		};
	}

	cancelAutosave() {
		if (this.autosaveTimer) clearTimeout(this.autosaveTimer);
		this.autosaveTimer = null;
	}

	buildSpawnBody(): SpawnRequest {
		return buildSpawnBody(this.effectiveForm, this.spawnProvider, envMap(this.envRows), this.followupParent);
	}
	draftBody(): SpawnRequest {
		return draftBody(this.buildSpawnBody(), this.envRows, this.files);
	}

	/** Whether the form holds anything the user would miss. */
	isDirty(): boolean {
		return (
			!!this.form.prompt.trim() ||
			!!this.form.name.trim() ||
			this.envRows.some((r) => r.key.trim()) ||
			this.files.length > 0
		);
	}
	/** Write the form to its draft row now; false when it can't be a draft yet. */
	flushDraft(): Promise<boolean> {
		return autosave(this);
	}

	private resetForm() {
		this.cancelAutosave();
		this.draftId = null;
		drafts.clear(this.slotKey);
		drafts.clear(SPAWN_SLOT);
		this.form = { ...blank, machine_id: this.form.machine_id, dispatcher: this.form.dispatcher };
		this.envRows = [];
		this.files = [];
		this.images.reset();
		this.oneOff = null;
	}
	discardMirror() {
		if (!this.draftId) return;
		const id = this.draftId;
		this.draftId = null;
		this.actions.discardDraft(id).catch(() => {});
	}
	/** The form is done with: reset it and hand back to the host. */
	finish() {
		this.resetForm();
		this.onspawned();
		this.onclose();
	}

	setTarget(value: string) {
		this.target = value === 'dispatch' ? 'dispatch' : 'machine';
	}

	addFiles = (incoming: File[]) => {
		if (this.busy) return;
		this.images.add(
			incoming,
			(file) => {
				({ files: this.files, text: this.form.prompt } = attachFiles(this.files, this.form.prompt, [file]));
			},
			(file) => toasts.error(m.attachments_compression_failed({ name: file.name }))
		);
	};
	removeFile = (name: string) => {
		this.files = removeFileByName(this.files, name);
	};

	async createProfile() {
		const base = this.profileSpec ?? specFromForm(this.form, this.allAccounts, this.allPools);
		const name = uniqueProfileName(
			m.spawn_profile_new_name(),
			this.profiles.map((p) => p.name)
		);
		try {
			const p = await this.profileActions.create({ name, ...base });
			this.selectedProfileId = p.id;
			this.oneOff = null;
		} catch (e) {
			toasts.error(m.spawn_profile_toast_failed({ error: errMessage(e) }));
		}
	}
	async saveProfile(id: string, name: string, spec: ProfileSpecForm) {
		try {
			await this.profileActions.update(id, { name, spec });
			toasts.ok(m.spawn_profile_toast_saved());
		} catch (e) {
			toasts.error(m.spawn_profile_toast_failed({ error: errMessage(e) }));
		}
	}
	async deleteProfile(id: string) {
		try {
			await this.profileActions.remove(id);
			if (this.selectedProfileId === id) {
				this.selectedProfileId = null;
				this.oneOff = null;
			}
		} catch (e) {
			toasts.error(m.spawn_profile_toast_failed({ error: errMessage(e) }));
		}
	}
	rememberProfileUse(p: SessionProfile | null) {
		if (!p) return;
		this.usageRaw = recordProfileUse(this.usageRaw, p.id);
		drafts.set(PROFILE_USES, this.usageRaw);
	}

	submit = async () => {
		if (!this.valid || this.busy) return;
		this.busy = true;
		promptHistory.push(this.form.prompt);
		try {
			if (this.target === 'machine') await spawnOnMachine(this);
			else await dispatchToK8s(this);
		} catch (e) {
			const msg = errMessage(e);
			toasts.error(
				this.target === 'machine'
					? m.spawn_toast_spawn_failed({ error: msg })
					: m.spawn_toast_dispatch_failed({ error: msg })
			);
		} finally {
			this.busy = false;
		}
	};

	submitDraft = async () => {
		if (!this.draftValid || this.busy) return;
		this.busy = true;
		try {
			await saveDraft(this);
		} catch (e) {
			toasts.error(m.spawn_toast_save_draft_failed({ error: errMessage(e) }));
		} finally {
			this.busy = false;
		}
	};

	clearForm = () => {
		this.discardMirror();
		this.resetForm();
		if (this.docked()) this.onclose();
	};
}
