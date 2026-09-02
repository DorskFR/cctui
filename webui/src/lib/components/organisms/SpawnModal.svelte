<script lang="ts">
	import { errMessage } from '$lib/api';
	import type { SpawnRequest } from '@bindings/SpawnRequest';
	import type { DispatchRequest } from '@bindings/DispatchRequest';
	import {
		useAllMachines,
		useDispatchers,
		useSessionActions,
		useRecentDirs,
		useAccounts,
		useLabels,
		endpoints
	} from '$lib/queries';
	import type { Label } from '@bindings/Label';
	import { ws } from '$lib/ws.svelte';
	import { toasts } from '$lib/toast.svelte';
	import {
		drafts,
		SPAWN_DRAFT,
		LAST_MACHINE,
		LAST_SPAWN_NAME,
		LAST_SPAWN_LABELS,
		nextSessionName,
		normalizeDir
	} from '$lib/drafts';
	import {
		machineMemoryKey,
		dispatchMemoryKey,
		applyMemory,
		dirPrefill,
		labelPrefill,
		memoryFieldsOf,
		entryFromForm,
		MACHINE_MEMORY_FIELDS,
		DISPATCH_MEMORY_FIELDS,
		type MemoryPatch
	} from '$lib/spawnMemory';
	import { appendFileTokens, mergeFiles, removeFileByName, fileCapError } from '$lib/attachments';
	import {
		AutoGrid,
		Badge,
		Button,
		Dropzone,
		FileButton,
		Icon,
		Modal,
		Tabs,
		type TabItem
	} from '@dorsk/tsumikit';
	import { clickOutside } from '$lib/clickOutside';
	import { dialogBackdropGuard } from '$lib/dialogBackdropGuard';
	import { labelTint, hueToColor } from '$lib/labels';
	import AttachmentList from '$lib/components/molecules/AttachmentList.svelte';
	import LabelMenu from '$lib/components/molecules/LabelMenu.svelte';
	import EnvSecretsField from './spawn/EnvSecretsField.svelte';
	import MachineFields from './spawn/MachineFields.svelte';
	import DispatchFields from './spawn/DispatchFields.svelte';
	import type { Form, Target } from './spawn/types';
	import {
		accountBacksAdapter,
		providerForAdapter,
		isCompatibleProvider,
		NO_ACCOUNT
	} from './spawn/options';
	import { settings, type SpawnDockSide } from '$lib/settings.svelte';
	import { SPAWN_DOCK_WIDTH } from '$lib/spawnDock.svelte';
	import { m } from '$lib/paraglide/messages';

	let {
		onclose,
		onspawned,
		prefill = null,
		docked = null
	}: {
		// Modal: close the dialog. Docked: the form is done with (spawned, saved
		// as draft, or cleared) — the parent remounts it so it reseeds exactly the
		// way a reopened modal would (draft gone, memory + name proposal replayed).
		onclose: () => void;
		onspawned: () => void;
		// Docked panel (Settings › New session): render the same form pinned to
		// one edge of the Sessions screen instead of inside a Modal.
		docked?: SpawnDockSide | null;
		// "New session from same script": seed the form from an
		// existing session's config (machine, dir, adapter, model). Overrides the
		// persisted draft so the dialog opens ready to re-dispatch.
		prefill?: Partial<Form> | null;
	} = $props();

	const machines = useAllMachines(() => true);
	const dispatchers = useDispatchers(() => true);
	const dispatcherIds = $derived(dispatchers.data ?? []);
	const canDispatch = $derived(dispatcherIds.length > 0);

	// Spawn target + form shape live in ./spawn/types. Machine / Dispatch render
	// as tabs; a prefill (re-dispatch) is always a machine spawn.
	let target = $state<Target>('machine');
	const targetTabs: TabItem[] = [
		{ id: 'machine', label: m.spawn_tab_machine() },
		{ id: 'dispatch', label: m.spawn_tab_dispatch() }
	];

	// The blank form is all-unset; the per-(machine, cwd) spawn memory
	// fills it in once a machine/cwd (or dispatcher/repo) is known.
	const blank: Form = {
		machine_id: '',
		adapter_id: 'claude-code',
		working_dir: '',
		name: '',
		prompt: '',
		// No hardcoded fallback: empty = "Default", so the account
		// default permission mode (else claude's own) applies unless overridden.
		permission_mode: '' as Form['permission_mode'],
		dispatcher: '',
		dispatch_adapter: 'claude-code',
		identity: '',
		repo: '',
		ticket: '',
		prompt_file: '',
		model_claude: '',
		model_codex: '',
		model_account: '',
		account: '',
		account_provider: '',
		effort_claude: '',
		effort_codex: '',
		timeout: '',
		labels: []
	};
	interface SpawnDraftPayload extends Partial<Form> {
		envRows?: EnvRow[];
	}
	let loadedDraft = false;
	let restoredEnvRows: EnvRow[] = [];
	let form = $state<Form>(load());
	// What the modal seeded (draft/prefill/defaults): the baseline the memory
	// effects compare against so an explicit user edit is never clobbered.
	// Deliberately a one-time snapshot, not reactive.
	// svelte-ignore state_referenced_locally
	const initialFields = memoryFieldsOf(form);
	// svelte-ignore state_referenced_locally
	const initialDir = form.working_dir;
	// svelte-ignore state_referenced_locally
	const initialLabels = [...form.labels];
	let labelsApplied: string[] | null = null;
	function load(): Form {
		try {
			const raw = drafts.get(SPAWN_DRAFT);
			const saved = raw ? (JSON.parse(raw) as SpawnDraftPayload) : {};
			loadedDraft = !!raw && !prefill;
			restoredEnvRows = Array.isArray(saved.envRows) ? saved.envRows : [];
			const { envRows: _envRows, ...savedForm } = saved;
			const seeded = { ...blank, ...savedForm, ...(prefill ?? {}) };
			// Fresh open (no draft to restore, no explicit prefill): propose the
			// last submitted session name with a bumped numeric suffix, so serial
			// spawns don't retype a label (toto → toto-2 → toto-3).
			if (!raw && !prefill) {
				const lastName = drafts.get(LAST_SPAWN_NAME);
				if (lastName) seeded.name = nextSessionName(lastName);
				// Default the label picker to the last-used set unless a
				// draft/prefill already carries labels.
				if (!savedForm.labels) {
					seeded.labels = (drafts.get(LAST_SPAWN_LABELS) ?? '').split(',').filter(Boolean);
				}
			}
			return seeded;
		} catch {
			return { ...blank, ...(prefill ?? {}) };
		}
	}

	// default the machine to the last used one (else the first) once loaded
	$effect(() => {
		const list = machines.data ?? [];
		if (form.machine_id || !list.length) return;
		const last = drafts.get(LAST_MACHINE);
		form.machine_id = last && list.some((m) => m.id === last) ? last : list[0].id;
	});

	// Spawn memory: the config last submitted for a (machine, cwd) —
	// account/harness/model/effort/permission mode/label — recalled from the
	// server-persisted settings blob whenever the machine or cwd changes, so a
	// new session on a known machine+cwd needs zero config clicks. Precedence:
	// an explicit edit in the open modal (including a restored draft or prefill)
	// wins over the memory, which wins over the blank-form seed.
	// `memApplied` tracks what the memory wrote so applyMemory can tell an edit
	// from its own writes. Keys are set BEFORE writing fields, so the re-runs
	// triggered by those writes hit the early-return.
	let memApplied: MemoryPatch = {};

	// Picking a machine pre-fills its most recent working dir (which then keys
	// the full memory recall below). Keyed on (machine, remembered dir) — not
	// the machine alone — so the fill re-attempts when the settings blob
	// hydrates after the machine was already picked. A draft/prefill only
	// suppresses the fill when it actually carries a dir: a stale draft saved
	// with an empty cwd must not pin the field empty forever.
	let dirComboApplied: string | null = null;
	let dirApplied: string | null = null;
	$effect(() => {
		const id = form.machine_id;
		if (!id) return;
		// Spawn memory first; on a machine this browser has never spawned on, the
		// server's recent dirs still know where sessions run.
		const last = settings.lastDirFor(id) ?? recentDirs[0] ?? null;
		const combo = machineMemoryKey(id, last ?? '');
		if (combo === dirComboApplied) return;
		const first = dirComboApplied === null;
		dirComboApplied = combo;
		if (first && (prefill?.working_dir || (loadedDraft && initialDir))) return;
		const next = dirPrefill(form.working_dir, last, dirApplied ?? initialDir);
		if (next === null) return;
		dirApplied = next;
		form.working_dir = next;
	});

	let memKeyApplied = $state<string | null>(null);
	$effect(() => {
		const key = form.machine_id ? machineMemoryKey(form.machine_id, form.working_dir) : null;
		if (!key || key === memKeyApplied) return;
		const first = memKeyApplied === null;
		memKeyApplied = key;
		if (first && (prefill || loadedDraft)) return;
		// An unknown cwd on a known machine still inherits that machine's last
		// config rather than dropping back to the blank form.
		const entry = settings.recallSpawn(key) ?? settings.lastEntryFor(form.machine_id);
		if (!entry) return;
		const patch = applyMemory(
			MACHINE_MEMORY_FIELDS,
			form,
			initialFields,
			memApplied,
			entry,
			drafts.get(LAST_SPAWN_NAME)
		);
		memApplied = { ...memApplied, ...patch };
		Object.assign(form, patch as Partial<Form>);
		const labels = labelPrefill(form.labels, initialLabels, labelsApplied, entry);
		if (labels) {
			labelsApplied = labels;
			form.labels = labels;
		}
	});

	// Dispatch flavor keyed by (dispatcher, repo): same memory, claude-family
	// knobs only (a dispatched worker is always claude).
	let dispatchKeyApplied = $state<string | null>(null);
	$effect(() => {
		const key = form.dispatcher ? dispatchMemoryKey(form.dispatcher, form.repo) : null;
		if (!key || key === dispatchKeyApplied) return;
		const first = dispatchKeyApplied === null;
		dispatchKeyApplied = key;
		if (first && (prefill || loadedDraft)) return;
		const entry = settings.recallSpawn(key);
		if (!entry) return;
		const patch = applyMemory(
			DISPATCH_MEMORY_FIELDS,
			form,
			initialFields,
			memApplied,
			entry,
			drafts.get(LAST_SPAWN_NAME)
		);
		memApplied = { ...memApplied, ...patch };
		Object.assign(form, patch as Partial<Form>);
	});

	// default the dispatcher to the first configured one once loaded
	$effect(() => {
		if (form.dispatcher || !dispatcherIds.length) return;
		form.dispatcher = dispatcherIds[0];
	});

	// recent working dirs on the selected machine, from the server (last 5).
	const dirsQuery = useRecentDirs(() => form.machine_id);
	// Collapse `folder` and `folder/` into one canonical `folder` entry.
	const recentDirs = $derived([...new Set((dirsQuery.data ?? []).map(normalizeDir))]);

	// Working-directory autocomplete lives in the MachineFields FilterInput
	// (spawn/cwdSchema.ts), fed the recent dirs below.

	// OAuth accounts. The picker offers only accounts whose provider
	// matches the selected adapter (codex → openai, else anthropic). Switching
	// adapter to one with no matching account clears the stale selection.
	const accounts = useAccounts(() => true);

	// Labels: the picker selects label ids into `form.labels`; on spawn
	// they're attached to the new session and remembered for next time. New
	// labels are created server-side immediately (get-or-create) so we always
	// track real ids. Display resolves ids against the live label set, dropping
	// any that were deleted.
	const labelsQuery = useLabels();
	const allLabels = $derived(labelsQuery.data?.labels ?? []);
	const selectedLabels = $derived(allLabels.filter((l) => form.labels.includes(l.id)));
	async function createSpawnLabel(name: string, color: string): Promise<Label> {
		return actions.createLabel(name, color);
	}
	function attachSpawnLabel(id: string) {
		if (!form.labels.includes(id)) form.labels = [...form.labels, id];
	}
	function detachSpawnLabel(id: string) {
		form.labels = form.labels.filter((x) => x !== id);
	}
	// "Add label" picker (the shared LabelMenu panel): toggle attach/detach and
	// create-and-attach, mirroring LabelBadge's editable behaviour but driven
	// from the add-ons action row instead of an inline tag trigger.
	// "Add label" dropdown. The panel uses the native popover API so the platform
	// renders it in the TOP LAYER — above the Modal's native <dialog> and outside
	// its scrolling body. (A plain absolute/fixed menu would either grow the
	// modal's scroll area and spawn scrollbars, or render behind the dialog.) We
	// place it from the trigger's rect.
	let labelMenuOpen = $state(false);
	let labelTriggerEl = $state<HTMLElement | null>(null);
	let labelMenuEl = $state<HTMLElement | null>(null);
	let labelMenuPos = $state({ top: 0, left: 0 });
	function openLabelMenu() {
		if (!labelTriggerEl) return;
		const r = labelTriggerEl.getBoundingClientRect();
		// Initial guess (below the trigger); refined once the panel has a measured
		// height so we can flip it above when it would overflow the viewport.
		labelMenuPos = { top: r.bottom + 4, left: r.left };
		labelMenuOpen = true;
		labelMenuEl?.showPopover();
		requestAnimationFrame(placeLabelMenu);
	}
	// Place the open panel: below the trigger by default, flipped above when it
	// wouldn't fit below; clamped into the viewport horizontally.
	function placeLabelMenu() {
		if (!labelTriggerEl || !labelMenuEl) return;
		const gap = 4;
		const t = labelTriggerEl.getBoundingClientRect();
		const m = labelMenuEl.getBoundingClientRect();
		const spaceBelow = window.innerHeight - t.bottom;
		const flipUp = spaceBelow < m.height + gap && t.top > spaceBelow;
		const top = flipUp ? Math.max(gap, t.top - m.height - gap) : t.bottom + gap;
		const left = Math.max(gap, Math.min(t.left, window.innerWidth - m.width - gap));
		labelMenuPos = { top, left };
	}
	function closeLabelMenu() {
		if (!labelMenuOpen) return;
		labelMenuOpen = false;
		labelMenuEl?.hidePopover();
	}
	const toggleLabelMenu = () => (labelMenuOpen ? closeLabelMenu() : openLabelMenu());
	const attachedLabelIds = $derived(new Set(form.labels));
	function toggleSpawnLabel(l: Label) {
		if (form.labels.includes(l.id)) detachSpawnLabel(l.id);
		else attachSpawnLabel(l.id);
	}
	async function createAndAttachSpawnLabel(name: string) {
		if (!name.trim()) return;
		const label = await createSpawnLabel(name, hueToColor(null));
		attachSpawnLabel(label.id);
	}

	// Attach the chosen labels to a session once we know its id.
	async function attachLabelsTo(sessionId: string, ids: string[]) {
		for (const id of ids) {
			try {
				await actions.attachLabel(sessionId, id);
			} catch {
				/* best-effort: a deleted label or transient error shouldn't fail the spawn */
			}
		}
	}

	// Machine spawns don't return a session id (the worker registers its own id
	// later — see spawn.rs). Correlate with the documented heuristic: the newest
	// session on the same (machine, working_dir) registered at/after the request,
	// retrying briefly while the worker comes up.
	async function attachLabelsToSpawned(
		machineId: string,
		cwd: string,
		sinceMs: number,
		ids: string[]
	) {
		if (!ids.length) return;
		for (let i = 0; i < 6; i++) {
			let list;
			try {
				list = await endpoints.sessions(false);
			} catch {
				return;
			}
			const match = list.sessions
				.filter((s) => s.machine_id === machineId && s.working_dir === cwd)
				.filter((s) => !s.registered_at || new Date(s.registered_at).getTime() >= sinceMs - 2000)
				.sort(
					(a, b) =>
						new Date(b.registered_at ?? 0).getTime() - new Date(a.registered_at ?? 0).getTime()
				)[0];
			if (match) {
				await attachLabelsTo(match.id, ids);
				return;
			}
			await new Promise((r) => setTimeout(r, 500));
		}
	}

	// Account is the primary axis: MachineFields offers every account
	// and derives the allowed harnesses + model list from the chosen one.
	// Stale-selection cleanup lives in MachineFields. Accounts are identities
	//: matched by name; the credential in play is the provider whose
	// family backs the effective harness.
	const allAccounts = $derived(accounts.data ?? []);
	const selectedAccount = $derived(
		form.account && form.account !== NO_ACCOUNT
			? allAccounts.find((a) => a.name === form.account)
			: undefined
	);
	// The submitted harness is always the user's pick: never the
	// silently-swapped effective adapter, which regressed codex to claude-code.
	const effectiveAdapter = $derived(form.adapter_id);
	const spawnProvider = $derived(providerForAdapter(selectedAccount, effectiveAdapter)?.provider);
	// A named account that can't back the picked harness blocks the spawn with a
	// visible error (MachineFields) instead of submitting the wrong harness.
	const harnessValid = $derived(accountBacksAdapter(selectedAccount, form.adapter_id));
	// Dispatch gateway routing uses the account's provider for the selected
	// dispatch harness: claude worker → anthropic family, codex worker
	// → openai family.
	const dispatchProvider = $derived(
		providerForAdapter(selectedAccount, form.dispatch_adapter || 'claude-code')?.provider
	);

	const actions = useSessionActions();
	let busy = $state(false);

	// --- Environment secrets & file uploads ---
	// Deliberately kept OUT of `form` (which is persisted to localStorage drafts)
	// so secret values and file handles are never written to disk — they live for
	// the modal's lifetime only and are fixed for the session once spawned.
	interface EnvRow {
		key: string;
		value: string;
	}
	let envRows = $state<EnvRow[]>(restoredEnvRows);
	let files = $state<File[]>([]);
	$effect(() => {
		drafts.set(SPAWN_DRAFT, JSON.stringify({ ...form, envRows }));
	});

	const ENV_KEY_RE = /^[A-Z_][A-Z0-9_]*$/;

	// Rows with a non-empty key whose key fails the shell-var pattern.
	const badEnvKeys = $derived(
		envRows.filter((r) => r.key.trim() && !ENV_KEY_RE.test(r.key.trim()))
	);
	const fileError = $derived(fileCapError(files));
	const secretsValid = $derived(badEnvKeys.length === 0 && !fileError);

	const addFiles = (incoming: File[]) => {
		files = mergeFiles(files, incoming);
		form.prompt = appendFileTokens(form.prompt, incoming);
	};
	const addEnvRow = () => (envRows = [...envRows, { key: '', value: '' }]);

	/** Collected env map: complete rows only (both key and value set). */
	function envMap(): Record<string, string> {
		const out: Record<string, string> = {};
		for (const r of envRows) {
			const k = r.key.trim();
			if (k && r.value) out[k] = r.value;
		}
		return out;
	}
	const removeFile = (name: string) => (files = removeFileByName(files, name));

	const spawnValid = $derived(!!form.machine_id && !!form.working_dir.trim() && harnessValid);
	// A dispatched worker needs a dispatcher and something to run (inline prompt
	// or a server-side prompt file). The repo is optional (the worker falls back
	// to its default cwd), but in practice you'll want one.
	const dispatchValid = $derived(
		!!form.dispatcher && (!!form.prompt.trim() || !!form.prompt_file.trim())
	);
	const valid = $derived((target === 'machine' ? spawnValid : dispatchValid) && secretsValid);

	// Build the SpawnRequest from the current machine-target form. Shared by the
	// immediate spawn and the "Save as draft" path; `save_draft` and
	// `env` are overridden by the caller as needed.
	function buildSpawnBody(): SpawnRequest {
		// The harness is always the user's pick.
		const adapter = form.adapter_id;
		// Explicit unbound spawn: the machine's own login, no gateway.
		const noAccount = form.account === NO_ACCOUNT;
		const compatible = !!spawnProvider && isCompatibleProvider(spawnProvider);
		const model = compatible
			? form.model_account || null
			: (adapter === 'codex' ? form.model_codex : form.model_claude) || null;
		return {
			machine_id: form.machine_id,
			working_dir: normalizeDir(form.working_dir.trim()),
			adapter_id: adapter,
			name: form.name.trim() || null,
			prompt: form.prompt.trim() || null,
			prompt_name: null,
			// Omit when unset: null lets the server resolve the account
			// default permission mode, else claude's own — never force a mode.
			permission_mode: form.permission_mode || null,
			effort: (adapter === 'codex' ? form.effort_codex : form.effort_claude) || null,
			model,
			env: envMap(),
			account: noAccount ? null : form.account.trim() || null,
			// The provider credential backing the chosen harness, so the
			// server resolves the exact credential under the account identity.
			provider: noAccount ? null : spawnProvider || null,
			no_account: noAccount,
			// "Auto" (the empty selection) delegates the choice: the server
			// binds whichever account has the most allocation left for this
			// model instead of refusing to pick between several.
			auto_account: !noAccount && !form.account.trim(),
			save_draft: false
		};
	}

	// Save the current form as a draft instead of dispatching. No env
	// is sent (re-entered at launch); the draft appears in the Drafts section.
	async function saveDraft() {
		const body: SpawnRequest = { ...buildSpawnBody(), env: {}, save_draft: true };
		await actions.spawn(body, []);
		drafts.set(LAST_MACHINE, form.machine_id);
		drafts.set(LAST_SPAWN_NAME, form.name.trim());
		toasts.ok(m.spawn_toast_saved_draft());
		drafts.clear(SPAWN_DRAFT);
		form = { ...blank, machine_id: form.machine_id, dispatcher: form.dispatcher };
		envRows = [];
		files = [];
		onspawned();
		onclose();
	}

	async function spawnOnMachine() {
		const body: SpawnRequest = buildSpawnBody();
		// Capture label intent before the form is reset on success.
		const labelIds = [...form.labels];
		const labelCwd = normalizeDir(form.working_dir.trim());
		const labelMachine = form.machine_id;
		const requestedAt = Date.now();
		const res = await actions.spawn(body, files);
		// Surface which credential the server bound — chiefly an
		// auto-bound default the user never named.
		if (res.account) toasts.push(m.spawn_toast_bound_account({ account: res.account }), 'info');
		drafts.set(LAST_MACHINE, form.machine_id);
		// An empty submitted name clears the proposal (drafts.set removes the key).
		drafts.set(LAST_SPAWN_NAME, form.name.trim());
		// Remember the label set for the next New Session (empty clears it).
		drafts.set(LAST_SPAWN_LABELS, labelIds.join(','));
		// Remember this config for (machine, cwd) so the next spawn
		// here pre-selects it. Saved on submit (not just on confirmed success)
		// so a slow/unconfirmed spawn still records the operator's intent.
		settings.rememberSpawn(machineMemoryKey(labelMachine, labelCwd), entryFromForm(form));
		toasts.push(m.spawn_toast_spawning(), 'info');
		const result = await ws.awaitCommand(res.command_id);
		if (result.ok) {
			toasts.ok(m.spawn_toast_spawned());
			// Attach labels to the freshly-registered session (best-effort, async).
			void attachLabelsToSpawned(labelMachine, labelCwd, requestedAt, labelIds);
			drafts.clear(SPAWN_DRAFT);
			form = { ...blank, machine_id: form.machine_id, dispatcher: form.dispatcher };
			envRows = [];
			files = [];
			onspawned();
			onclose();
		} else if (result.timedOut) {
			// Still try to label it — the session usually lands shortly after.
			void attachLabelsToSpawned(labelMachine, labelCwd, requestedAt, labelIds);
			// No confirmation ≠ failed: slow/cold spawns routinely land
			// after the wait. Close + refresh so the new session shows up; keep the
			// draft so a *real* miss is one re-open away. Re-submitting blindly
			// would dispatch a second spawn → duplicate agent.
			toasts.push(m.spawn_toast_unconfirmed(), 'info');
			onspawned();
			onclose();
		} else {
			toasts.err(m.spawn_toast_spawn_failed({ error: result.error ?? m.spawn_error_unknown() }));
		}
	}

	// Stable across retries: on a dispatch failure the modal stays open, and
	// re-submitting reuses the same session_id so the server's idempotency
	// dedup makes it a genuine retry (not a second pod). Cleared on success.
	let pendingDispatchId = $state<string | null>(null);

	async function dispatchToK8s() {
		// Build the opaque payload the dispatcher unpacks into TASK_* env. Omit
		// empty fields so the worker's own defaults apply.
		const payload: Record<string, unknown> = {};
		if (form.name.trim()) payload.name = form.name.trim();
		if (form.identity.trim()) payload.identity = form.identity.trim();
		if (form.repo.trim()) payload.repo = form.repo.trim();
		// A ticket id becomes the flow's context — the worker exports `context` as
		// TASK_CONTEXT_JSON and the prompt reads `issue_id` from it.
		if (form.ticket.trim()) payload.context = { issue_id: form.ticket.trim() };
		if (form.prompt.trim()) payload.prompt = form.prompt.trim();
		if (form.prompt_file.trim()) payload.prompt_file = form.prompt_file.trim();
		// Adapter choice: omit for the claude-code default so an older
		// server/worker stays backward compatible; set it for a codex worker.
		const dispatchAdapter = form.dispatch_adapter || 'claude-code';
		if (dispatchAdapter === 'codex') payload.adapter = 'codex';
		// The model is account-driven for a compatible account, else the
		// per-adapter family field.
		const dispatchCompatible = !!dispatchProvider && isCompatibleProvider(dispatchProvider);
		const dispatchModel = dispatchCompatible
			? form.model_account.trim()
			: dispatchAdapter === 'codex'
				? form.model_codex.trim()
				: form.model_claude.trim();
		if (dispatchModel) payload.model = dispatchModel;
		const dispatchEffort =
			dispatchAdapter === 'codex' ? form.effort_codex.trim() : form.effort_claude.trim();
		if (dispatchEffort) payload.effort = dispatchEffort;
		// Environment secrets: the external dispatcher turns `env` into
		// pod env / an ephemeral Secret. The server redacts these from its dispatch
		// notifications and never persists them.
		const env = envMap();
		if (Object.keys(env).length) payload.env = env;
		const timeout = form.timeout.trim() ? Number(form.timeout.trim()) : null;
		// Client-minted id doubles as the idempotency key; held stable
		// across retries so a re-submit dedups to the same session.
		pendingDispatchId ??= crypto.randomUUID();
		const body: DispatchRequest = {
			dispatcher: form.dispatcher,
			session_id: pendingDispatchId,
			timeout: Number.isFinite(timeout) ? timeout : null,
			reply_url: null,
			// Server-side completion webhook is not driven from the
			// spawn modal; leave unset so the dispatch contract is satisfied.
			notify_url: null,
			notify_secret: null,
			// Account routing on the dispatch path: the server mints the
			// gateway token + merges its base-url/token into payload.env. The
			// no-account sentinel is a machine-tab concept → treat as null.
			account: form.account === NO_ACCOUNT ? null : form.account.trim() || null,
			provider: dispatchProvider || null,
			// Multi-account routing isn't driven from the modal; the
			// singular account/provider pair above is the modal's contract.
			accounts: [],
			// `payload` is opaque (JsonValue) server-side; our local shape carries a
			// nested `env` object, so cast at the boundary.
			payload: payload as DispatchRequest['payload']
		};
		const labelIds = [...form.labels];
		const dispatchedId = pendingDispatchId;
		const res = await actions.dispatch(body);
		drafts.set(LAST_SPAWN_NAME, form.name.trim());
		// Remember the dispatch flavor for (dispatcher, repo).
		settings.rememberSpawn(dispatchMemoryKey(form.dispatcher, form.repo), entryFromForm(form));
		// Remember the label set + attach to the dispatched session (its id is
		// the client-minted dispatch id).
		drafts.set(LAST_SPAWN_LABELS, labelIds.join(','));
		void attachLabelsTo(dispatchedId, labelIds);
		toasts.ok(m.spawn_toast_dispatched({ dispatcher: res.dispatcher, handle: res.handle }));
		pendingDispatchId = null;
		drafts.clear(SPAWN_DRAFT);
		form = { ...blank, machine_id: form.machine_id, dispatcher: form.dispatcher };
		envRows = [];
		files = [];
		onspawned();
		onclose();
	}

	async function submit() {
		if (!valid || busy) return;
		busy = true;
		try {
			if (target === 'machine') await spawnOnMachine();
			else await dispatchToK8s();
		} catch (e) {
			const msg = errMessage(e);
			toasts.err(
				target === 'machine'
					? m.spawn_toast_spawn_failed({ error: msg })
					: m.spawn_toast_dispatch_failed({ error: msg })
			);
		} finally {
			busy = false;
		}
	}

	// Drafts are a machine-spawn concept; a draft only needs a target +
	// prompt, not the full dispatch contract, so it's valid whenever the spawn
	// form is. Buffer-only, so secrets needn't be valid yet (entered at launch).
	const draftValid = $derived(target === 'machine' && spawnValid);
	async function submitDraft() {
		if (!draftValid || busy) return;
		busy = true;
		try {
			await saveDraft();
		} catch (e) {
			toasts.err(m.spawn_toast_save_draft_failed({ error: errMessage(e) }));
		} finally {
			busy = false;
		}
	}

	function clearForm() {
		form = { ...blank, machine_id: form.machine_id, dispatcher: form.dispatcher };
		envRows = [];
		files = [];
		drafts.clear(SPAWN_DRAFT);
		// Docked: a cleared form reseeds like a fresh modal would (memory-filled
		// config, proposed name) instead of sitting blank until the next visit.
		if (docked) onclose();
	}
</script>

<!-- The form body and the action row are snippets so the Modal and the docked
     panel render the very same markup; only the chrome around them differs. -->
{#if docked}
	<aside
		class="dock"
		class:dock-left={docked === 'left'}
		aria-label={m.spawn_modal_title()}
		style:--spawn-dock-w={SPAWN_DOCK_WIDTH}
	>
		<div class="dock-head">{m.spawn_modal_title()}</div>
		<div class="dock-body">{@render body()}</div>
		<div class="dock-foot">{@render footer()}</div>
	</aside>
{:else}
	<Modal title={m.spawn_modal_title()} {onclose} resizeKey="cctui_spawn_modal_width" {body} {footer} />
{/if}

{#snippet body()}
	<!-- The whole dialog is a file drop area: dragging files over it
	     shows the tsumikit Dropzone overlay; on drop they're staged as
	     attachments. overlay mode wraps the content without hijacking clicks,
	     and is disabled on the dispatch target (no attachments there). -->
	<Dropzone
		overlay
		multiple
		label={m.spawn_dropzone_label()}
		disabled={target !== 'machine'}
		onfiles={addFiles}
	>
		<div class="stack" use:dialogBackdropGuard>
		{#snippet machineFields()}
			<MachineFields
				bind:form
				machines={machines.data ?? []}
				{recentDirs}
				accounts={allAccounts}
				onsubmit={submit}
				onfiles={addFiles}
				docked={!!docked}
			/>
		{/snippet}
		{#snippet targetPanel(id: string)}
			<div class="stack">
				{#if id === 'dispatch'}
					<DispatchFields bind:form {dispatcherIds} accounts={allAccounts} onsubmit={submit} />
				{:else}
					{@render machineFields()}
				{/if}
			</div>
		{/snippet}
		<!-- Machine / Dispatch are tabs when a dispatcher exists. -->
		{#if canDispatch}
			<Tabs
				label={m.spawn_run_on_label()}
				tabs={targetTabs}
				bind:value={() => target as string, (v) => (target = v === 'dispatch' ? 'dispatch' : 'machine')}
				panel={targetPanel}
			/>
		{:else}
			{@render machineFields()}
		{/if}

		<!-- Shared add-ons: one row of equal-width buttons — Add label
		     · Add files (machine only) · Add env vars. Each control's
		     content renders below in the same order
		     (labels, files, env vars). All three are the canonical Button look
		     (a FileButton matches the Button atom by design); two carry an
		     icon. Grid blockifies the items so each fills its 1fr column. -->
		<div class="addons">
			<span class="addon-title">{m.spawn_optional_settings()}</span>
			<AutoGrid min="8rem" gap="var(--sp-2)" maxCols={3} align="stretch">
				<div
					class="label-add"
					bind:this={labelTriggerEl}
					use:clickOutside={closeLabelMenu}
				>
					<Button
						block
						aria-haspopup="true"
						aria-expanded={labelMenuOpen}
						onclick={toggleLabelMenu}
					>
						<Icon name="tag" />{m.spawn_add_label()}
					</Button>
					<!-- svelte-ignore a11y_no_static_element_interactions -->
					<div
						bind:this={labelMenuEl}
						class="label-menu"
						popover="manual"
						role="menu"
						aria-label={m.spawn_labels_aria()}
						tabindex="-1"
						style:top="{labelMenuPos.top}px"
						style:left="{labelMenuPos.left}px"
						onkeydown={(e) => {
							if (e.key === 'Escape') closeLabelMenu();
						}}
					>
						{#if labelMenuOpen}
							<LabelMenu
								labels={allLabels}
								selectedIds={attachedLabelIds}
								cap={5}
								autofocus
								onToggle={toggleSpawnLabel}
								onCreate={createAndAttachSpawnLabel}
								onUpdate={(labelId, patch) => actions.updateLabel(labelId, patch)}
								onDelete={(labelId) => actions.deleteLabel(labelId)}
							/>
						{/if}
					</div>
				</div>
				{#if target === 'machine'}
					<FileButton label={m.spawn_add_files()} icon="file-text" multiple onfiles={addFiles} />
				{/if}
				<Button block onclick={addEnvRow}><Icon name="plus" />{m.spawn_add_env_vars()}</Button>
			</AutoGrid>

			<!-- Each add-on's content, rendered where due: labels, files, env. -->
			{#if selectedLabels.length}
				<div class="addon-labels">
					{#each selectedLabels as l (l.id)}
						<Badge
							style="{labelTint(l)};border-radius:var(--r-sm)"
							removable
							onremove={() => detachSpawnLabel(l.id)}
						>
							{l.name}
						</Badge>
					{/each}
				</div>
			{/if}
			{#if target === 'machine'}
				<AttachmentList {files} onremove={removeFile} />
			{/if}
			<EnvSecretsField bind:envRows invalid={badEnvKeys.length > 0} />
		</div>
		</div>
	</Dropzone>
{/snippet}
{#snippet footer()}
	<Button size="lg" onclick={clearForm}>{m.spawn_clear()}</Button>
	{#if target === 'machine'}
		<Button size="lg" disabled={busy || !draftValid} onclick={submitDraft}>
			{m.spawn_save_draft()}
		</Button>
	{/if}
	<Button size="lg" variant="primary" block disabled={busy || !valid} onclick={submit}>
		{#if busy}<span class="spin"></span>{:else}{target === 'machine' ? m.spawn_action_spawn() : m.spawn_action_dispatch()}{/if}
	</Button>
{/snippet}

<style>
	/* Docked panel: pinned to one edge between the header and the bottom nav,
	   scrolling its body on its own. The layout reserves the same width. */
	.dock {
		position: fixed;
		top: calc(var(--header-h) + var(--safe-top));
		bottom: calc(var(--nav-h) + var(--safe-bottom));
		right: 0;
		width: var(--spawn-dock-w);
		display: flex;
		flex-direction: column;
		background: var(--bg-elevated);
		border-left: 1px solid var(--border);
		z-index: 4;
	}
	.dock.dock-left {
		right: auto;
		left: 0;
		border-left: 0;
		border-right: 1px solid var(--border);
	}
	.dock-head {
		flex: none;
		padding: var(--sp-3) var(--sp-3) var(--sp-2);
		font-weight: var(--fw-semibold);
		border-bottom: 1px solid var(--border);
	}
	.dock-body {
		flex: 1;
		min-height: 0;
		overflow-y: auto;
		padding: var(--sp-3);
	}
	.dock-foot {
		flex: none;
		display: flex;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
		border-top: 1px solid var(--border);
	}
	/* Add-ons: a single wrapping row of "Add …" buttons, with each control's
	   content stacked below. */
	.addons {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.addon-title {
		font-size: var(--fs-sm);
		font-weight: var(--fw-medium);
		color: var(--text-muted);
	}
	.addon-labels {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-1);
	}
	/* "Add label" owns its own popover (a real Button trigger + a clickOutside
	   menu, like LabelFilter) so it stays a plain Button — no Popover-trigger
	   restyling. The wrapper is the grid item; the Button fills it (block). */
	/* The grid stretches its cells (align="stretch"), so the wrapper — and the
	   Button inside it — fill the row height, lining up with the FileButton and
	   env-vars Button despite their differing min-height tokens. */
	.label-add {
		display: flex;
		align-items: stretch;
	}
	/* Native popover: the browser renders it in the top layer (above the modal's
	   <dialog>, outside its scrolling body). Reset the UA popover defaults
	   (centered, bordered) and place it from the trigger rect (top/left inline). */
	.label-menu {
		position: fixed;
		inset: auto;
		margin: 0;
		padding: var(--sp-1);
		display: flex;
		flex-direction: column;
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		background: var(--bg-elevated);
		box-shadow: var(--shadow-lg, 0 8px 24px rgba(0, 0, 0, 0.4));
	}
	.label-menu:not(:popover-open) {
		display: none;
	}
</style>
