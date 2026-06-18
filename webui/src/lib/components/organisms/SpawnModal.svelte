<script lang="ts">
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
		loadMachinePrefs,
		saveMachinePrefs
	} from '$lib/drafts';
	import { mergeFiles, removeFileByName, fileCapError } from '$lib/attachments';
	import {
		AutoGrid,
		Badge,
		Button,
		Dropzone,
		FileButton,
		Field,
		Icon,
		Modal,
		OptionButton,
		Text
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
	import { adapterForProvider, isCompatibleProvider } from './spawn/options';

	let {
		onclose,
		onspawned,
		prefill = null
	}: {
		onclose: () => void;
		onspawned: () => void;
		// "New session from same script" (CCT-250 item 8): seed the form from an
		// existing session's config (machine, dir, adapter, model). Overrides the
		// persisted draft so the dialog opens ready to re-dispatch.
		prefill?: Partial<Form> | null;
	} = $props();

	const machines = useAllMachines(() => true);
	const dispatchers = useDispatchers(() => true);
	const dispatcherIds = $derived($dispatchers.data ?? []);
	const canDispatch = $derived(dispatcherIds.length > 0);

	// Spawn target + form shape live in ./spawn/types.
	let target = $state<Target>('machine');

	const blank: Form = {
		machine_id: '',
		adapter_id: 'claude-code',
		working_dir: '',
		name: '',
		prompt: '',
		permission_mode: 'yolo',
		dispatcher: '',
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
				// Default the label picker to the last-used set (CCT-360) unless a
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
		const list = $machines.data ?? [];
		if (form.machine_id || !list.length) return;
		const last = drafts.get(LAST_MACHINE);
		form.machine_id = list.some((m) => m.id === last) ? last : list[0].id;
	});

	// Remember spawn settings PER MACHINE (CCT-274): when the selected machine
	// changes, pull that machine's last-used adapter/model/effort/account and
	// working dir so the next spawn on e.g. dev1 re-selects what you usually
	// run there. An explicit
	// prefill (re-dispatch from an existing session) takes precedence — we don't
	// clobber it. We set `prefsLoadedFor` BEFORE writing the fields, so the
	// re-runs triggered by those writes hit the early-return.
	let prefsLoadedFor = $state<string | null>(null);
	$effect(() => {
		const id = form.machine_id;
		if (!id || id === prefsLoadedFor) return;
		prefsLoadedFor = id;
		if (prefill || loadedDraft) return;
		const p = loadMachinePrefs(id);
		if (!p) return;
		if (p.adapter_id) form.adapter_id = p.adapter_id;
		if (p.model_claude != null) form.model_claude = p.model_claude;
		if (p.model_codex != null) form.model_codex = p.model_codex;
		if (p.effort_claude != null) form.effort_claude = p.effort_claude;
		if (p.effort_codex != null) form.effort_codex = p.effort_codex;
		if (p.account != null) form.account = p.account;
		if (p.account_provider != null) form.account_provider = p.account_provider;
		if (p.model_account != null) form.model_account = p.model_account;
		if (p.working_dir) form.working_dir = p.working_dir;
	});

	// default the dispatcher to the first configured one once loaded
	$effect(() => {
		if (form.dispatcher || !dispatcherIds.length) return;
		form.dispatcher = dispatcherIds.includes(form.dispatcher)
			? form.dispatcher
			: dispatcherIds[0];
	});

	// recent working dirs on the selected machine, from the server (last 5).
	const dirsQuery = useRecentDirs(() => form.machine_id);
	const recentDirs = $derived([...new Set($dirsQuery.data ?? [])]);

	// Working-directory autocomplete lives in spawn/CwdCombo.svelte.

	// OAuth accounts (CCT-237). The picker offers only accounts whose provider
	// matches the selected adapter (codex → openai, else anthropic). Switching
	// adapter to one with no matching account clears the stale selection.
	const accounts = useAccounts(() => true);

	// Labels (CCT-360): the picker selects label ids into `form.labels`; on spawn
	// they're attached to the new session and remembered for next time. New
	// labels are created server-side immediately (get-or-create) so we always
	// track real ids. Display resolves ids against the live label set, dropping
	// any that were deleted.
	const labelsQuery = useLabels();
	const allLabels = $derived($labelsQuery.data?.labels ?? []);
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

	// Account is the primary axis (CCT-399): MachineFields offers every account
	// and derives the harness + model list from the chosen one (no per-adapter
	// pre-filter). Stale-selection cleanup lives in MachineFields.
	const allAccounts = $derived($accounts.data ?? []);
	const selectedAccount = $derived(
		form.account
			? (allAccounts.find(
					(a) => a.name === form.account && a.provider === form.account_provider
				) ?? allAccounts.find((a) => a.name === form.account))
			: undefined
	);

	const actions = useSessionActions();
	let busy = $state(false);

	// --- Environment secrets (CCT-202) & file uploads (CCT-203) ---
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

	const addFiles = (incoming: File[]) => (files = mergeFiles(files, incoming));
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

	const spawnValid = $derived(!!form.machine_id && !!form.working_dir.trim());
	// A dispatched worker needs a dispatcher and something to run (inline prompt
	// or a server-side prompt file). The repo is optional (the worker falls back
	// to its default cwd), but in practice you'll want one.
	const dispatchValid = $derived(
		!!form.dispatcher && (!!form.prompt.trim() || !!form.prompt_file.trim())
	);
	const valid = $derived((target === 'machine' ? spawnValid : dispatchValid) && secretsValid);

	async function spawnOnMachine() {
		// The account drives the harness + model (CCT-399); "Default" falls back
		// to the adapter-first selection.
		const adapter = selectedAccount ? adapterForProvider(selectedAccount.provider) : form.adapter_id;
		const compatible = !!selectedAccount && isCompatibleProvider(selectedAccount.provider);
		const model = compatible
			? form.model_account || null
			: (adapter === 'codex' ? form.model_codex : form.model_claude) || null;
		const body: SpawnRequest = {
			machine_id: form.machine_id,
			working_dir: form.working_dir.trim(),
			adapter_id: adapter,
			name: form.name.trim() || null,
			prompt: form.prompt.trim() || null,
			prompt_name: null,
			permission_mode: form.permission_mode,
			effort: (adapter === 'codex' ? form.effort_codex : form.effort_claude) || null,
			model,
			env: envMap(),
			account: form.account.trim() || null,
			// Disambiguate a name shared across providers so the server resolves
			// the exact account (CCT-399).
			provider: selectedAccount?.provider || null
		};
		// Capture label intent before the form is reset on success (CCT-360).
		const labelIds = [...form.labels];
		const labelCwd = form.working_dir.trim();
		const labelMachine = form.machine_id;
		const requestedAt = Date.now();
		const res = await actions.spawn(body, files);
		drafts.set(LAST_MACHINE, form.machine_id);
		// An empty submitted name clears the proposal (drafts.set removes the key).
		drafts.set(LAST_SPAWN_NAME, form.name.trim());
		// Remember the label set for the next New Session (empty clears it).
		drafts.set(LAST_SPAWN_LABELS, labelIds.join(','));
		// Remember these settings for this machine (CCT-274) so the next spawn
		// here pre-selects them. Saved on submit (not just on confirmed success)
		// so a slow/unconfirmed spawn still records the operator's intent.
		saveMachinePrefs(form.machine_id, {
			adapter_id: form.adapter_id,
			model_claude: form.model_claude,
			model_codex: form.model_codex,
			effort_claude: form.effort_claude,
			effort_codex: form.effort_codex,
			account: form.account,
			account_provider: form.account_provider,
			model_account: form.model_account,
			working_dir: form.working_dir.trim()
		});
		toasts.push('Spawning…', 'info');
		const result = await ws.awaitCommand(res.command_id);
		if (result.ok) {
			toasts.ok('Session spawned');
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
			// No confirmation ≠ failed (CCT-242): slow/cold spawns routinely land
			// after the wait. Close + refresh so the new session shows up; keep the
			// draft so a *real* miss is one re-open away. Re-submitting blindly
			// would dispatch a second spawn → duplicate agent.
			toasts.push(
				'Spawn dispatched but not confirmed yet — check the list before retrying (a retry starts a second session)',
				'info'
			);
			onspawned();
			onclose();
		} else {
			toasts.err(`Spawn failed: ${result.error ?? 'unknown error'}`);
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
		// The model is account-driven for a compatible account (CCT-399), else the
		// claude family.
		const dispatchCompatible = !!selectedAccount && isCompatibleProvider(selectedAccount.provider);
		const dispatchModel = dispatchCompatible ? form.model_account.trim() : form.model_claude.trim();
		if (dispatchModel) payload.model = dispatchModel;
		if (form.effort_claude.trim()) payload.effort = form.effort_claude.trim();
		// Environment secrets (CCT-202): the external dispatcher turns `env` into
		// pod env / an ephemeral Secret. The server redacts these from its dispatch
		// notifications and never persists them.
		const env = envMap();
		if (Object.keys(env).length) payload.env = env;
		const timeout = form.timeout.trim() ? Number(form.timeout.trim()) : null;
		// Client-minted id doubles as the idempotency key (CCT-107); held stable
		// across retries (CCT-193) so a re-submit dedups to the same session.
		pendingDispatchId ??= crypto.randomUUID();
		const body: DispatchRequest = {
			dispatcher: form.dispatcher,
			session_id: pendingDispatchId,
			timeout: Number.isFinite(timeout) ? timeout : null,
			reply_url: null,
			// Account routing on the dispatch path (CCT-399): the server mints the
			// gateway token + merges its base-url/token into payload.env.
			account: form.account.trim() || null,
			provider: selectedAccount?.provider || null,
			// `payload` is opaque (JsonValue) server-side; our local shape carries a
			// nested `env` object, so cast at the boundary.
			payload: payload as DispatchRequest['payload']
		};
		const labelIds = [...form.labels];
		const dispatchedId = pendingDispatchId;
		const res = await actions.dispatch(body);
		drafts.set(LAST_SPAWN_NAME, form.name.trim());
		// Remember the label set + attach to the dispatched session (its id is the
		// client-minted dispatch id, CCT-360).
		drafts.set(LAST_SPAWN_LABELS, labelIds.join(','));
		void attachLabelsTo(dispatchedId, labelIds);
		toasts.ok(`Dispatched to ${res.dispatcher} (${res.handle})`);
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
			toasts.err(`${target === 'machine' ? 'Spawn' : 'Dispatch'} failed: ${(e as Error).message}`);
		} finally {
			busy = false;
		}
	}

	function clearForm() {
		form = { ...blank, machine_id: form.machine_id, dispatcher: form.dispatcher };
		envRows = [];
		files = [];
		drafts.clear(SPAWN_DRAFT);
	}
</script>

<Modal title="New session" {onclose} resizeKey="cctui_spawn_modal_width">
	{#snippet body()}
		<!-- The whole dialog is a file drop area (CCT-236): dragging files over it
		     shows the tsumikit Dropzone overlay; on drop they're staged as
		     attachments. overlay mode wraps the content without hijacking clicks,
		     and is disabled on the dispatch target (no attachments there). -->
		<Dropzone
			overlay
			multiple
			label="Drop files to attach"
			disabled={target !== 'machine'}
			onfiles={addFiles}
		>
			<div class="stack" use:dialogBackdropGuard>
			{#if canDispatch}
				<Field label="Run on">
					<div class="targets">
						<OptionButton
							selected={target === 'machine'}
							style="--opt-accent: var(--c-blue)"
							onclick={() => (target = 'machine')}
						>
							<strong>Machine</strong>
							<Text tone="faint" size="xs">An enrolled daemon</Text>
						</OptionButton>
						<OptionButton
							selected={target === 'dispatch'}
							style="--opt-accent: var(--c-blue)"
							onclick={() => (target = 'dispatch')}
						>
							<strong>Dispatch (k8s)</strong>
							<Text tone="faint" size="xs">Ephemeral worker pod</Text>
						</OptionButton>
					</div>
				</Field>
			{/if}

			{#if target === 'dispatch'}
				<DispatchFields bind:form {dispatcherIds} accounts={allAccounts} onsubmit={submit} />
			{:else}
				<MachineFields
					bind:form
					machines={$machines.data ?? []}
					{recentDirs}
					accounts={allAccounts}
					onsubmit={submit}
				/>
			{/if}

			<!-- Shared add-ons: one row of equal-width buttons — Add label
			     (CCT-360) · Add files (CCT-203, machine only) · Add env vars
			     (CCT-202). Each control's content renders below in the same order
			     (labels, files, env vars). All three are the canonical Button look
			     (a FileButton matches the Button atom by design); two carry an
			     icon. Grid blockifies the items so each fills its 1fr column. -->
			<div class="addons">
				<span class="addon-title">Optional settings</span>
				<AutoGrid min="8rem" gap="var(--sp-2)" maxCols={3}>
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
							<Icon name="tag" />Add label
						</Button>
						<!-- svelte-ignore a11y_no_static_element_interactions -->
						<div
							bind:this={labelMenuEl}
							class="label-menu"
							popover="manual"
							role="menu"
							aria-label="Labels"
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
						<FileButton label="Add files" icon="file-text" multiple onfiles={addFiles} />
					{/if}
					<Button onclick={addEnvRow}><Icon name="plus" />Add env vars</Button>
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
		<Button size="lg" onclick={clearForm}>Clear</Button>
		<Button size="lg" variant="primary" block disabled={busy || !valid} onclick={submit}>
			{#if busy}<span class="spin"></span>{:else}{target === 'machine' ? 'Spawn' : 'Dispatch'}{/if}
		</Button>
	{/snippet}
</Modal>

<style>
	.targets {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: var(--sp-2);
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
	.label-add {
		display: flex;
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
