<script lang="ts">
	import type { SpawnRequest } from '@bindings/SpawnRequest';
	import type { DispatchRequest } from '@bindings/DispatchRequest';
	import {
		useAllMachines,
		useDispatchers,
		useSessionActions,
		useRecentDirs,
		useAccounts
	} from '$lib/queries';
	import { ws } from '$lib/ws.svelte';
	import { toasts } from '$lib/toast.svelte';
	import {
		drafts,
		SPAWN_DRAFT,
		LAST_MACHINE,
		LAST_SPAWN_NAME,
		nextSessionName,
		loadMachinePrefs,
		saveMachinePrefs
	} from '$lib/drafts';
	import { dropzone } from '$lib/dropzone';
	import { mergeFiles, removeFileByName, fileCapError } from '$lib/attachments';
	import Modal from '$lib/components/molecules/Modal.svelte';
	import { Button, Field, OptionButton, Text } from '@dorsk/tsumikit';
	import EnvSecretsField from './spawn/EnvSecretsField.svelte';
	import MachineFields from './spawn/MachineFields.svelte';
	import DispatchFields from './spawn/DispatchFields.svelte';
	import type { Form, Target } from './spawn/types';

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
		account: '',
		effort_claude: '',
		effort_codex: '',
		timeout: ''
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
	const wantProvider = $derived(form.adapter_id === 'codex' ? 'openai' : 'anthropic');
	const matchingAccounts = $derived(
		($accounts.data ?? []).filter((a) => a.provider === wantProvider)
	);
	$effect(() => {
		if (form.account && !matchingAccounts.some((a) => a.name === form.account)) {
			form.account = '';
		}
	});

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

	// Highlight the modal as a dropzone while a file drag hovers it (CCT-236).
	let dragActive = $state(false);
	const addFiles = (incoming: File[]) => (files = mergeFiles(files, incoming));

	/** Collected env map: complete rows only (both key and value set). */
	function envMap(): Record<string, string> {
		const out: Record<string, string> = {};
		for (const r of envRows) {
			const k = r.key.trim();
			if (k && r.value) out[k] = r.value;
		}
		return out;
	}
	function onPickFiles(e: Event) {
		const picked = Array.from((e.currentTarget as HTMLInputElement).files ?? []);
		addFiles(picked);
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
		const body: SpawnRequest = {
			machine_id: form.machine_id,
			working_dir: form.working_dir.trim(),
			adapter_id: form.adapter_id,
			name: form.name.trim() || null,
			prompt: form.prompt.trim() || null,
			prompt_name: null,
			permission_mode: form.permission_mode,
			effort: (form.adapter_id === 'codex' ? form.effort_codex : form.effort_claude) || null,
			model: (form.adapter_id === 'codex' ? form.model_codex : form.model_claude) || null,
			env: envMap(),
			account: form.account.trim() || null
		};
		const res = await actions.spawn(body, files);
		drafts.set(LAST_MACHINE, form.machine_id);
		// An empty submitted name clears the proposal (drafts.set removes the key).
		drafts.set(LAST_SPAWN_NAME, form.name.trim());
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
			working_dir: form.working_dir.trim()
		});
		toasts.push('Spawning…', 'info');
		const result = await ws.awaitCommand(res.command_id);
		if (result.ok) {
			toasts.ok('Session spawned');
			drafts.clear(SPAWN_DRAFT);
			form = { ...blank, machine_id: form.machine_id, dispatcher: form.dispatcher };
			envRows = [];
			files = [];
			onspawned();
			onclose();
		} else if (result.timedOut) {
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
		if (form.model_claude.trim()) payload.model = form.model_claude.trim();
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
			// `payload` is opaque (JsonValue) server-side; our local shape carries a
			// nested `env` object, so cast at the boundary.
			payload: payload as DispatchRequest['payload']
		};
		const res = await actions.dispatch(body);
		drafts.set(LAST_SPAWN_NAME, form.name.trim());
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
		<div
			class="stack"
			class:dropping={dragActive && target === 'machine'}
			use:dropzone={{
				onFiles: addFiles,
				onActive: (a) => (dragActive = a),
				disabled: target !== 'machine'
			}}
		>
			{#if dragActive && target === 'machine'}
				<div class="dropoverlay">Drop files to attach</div>
			{/if}
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
				<DispatchFields bind:form {dispatcherIds} onsubmit={submit} />
			{:else}
				<MachineFields
					bind:form
					machines={$machines.data ?? []}
					{recentDirs}
					{matchingAccounts}
					{files}
					onpickfiles={onPickFiles}
					onremovefile={removeFile}
					onsubmit={submit}
				/>
			{/if}

			<!-- Environment secrets (CCT-202), both targets. -->
			<EnvSecretsField bind:envRows invalid={badEnvKeys.length > 0} />
		</div>
	{/snippet}
	{#snippet footer()}
		<Button onclick={clearForm}>Clear</Button>
		<Button variant="primary" block disabled={busy || !valid} onclick={submit}>
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
	.stack {
		position: relative;
	}
	.stack.dropping {
		outline: 2px dashed var(--c-blue);
		outline-offset: 4px;
		border-radius: var(--r-md);
	}
	.dropoverlay {
		position: absolute;
		inset: 0;
		z-index: 5;
		display: flex;
		align-items: center;
		justify-content: center;
		font-weight: var(--fw-medium);
		color: var(--c-blue);
		background: color-mix(in srgb, var(--c-blue) 12%, var(--bg));
		border-radius: var(--r-md);
		pointer-events: none;
	}
</style>
