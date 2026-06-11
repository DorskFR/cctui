<script lang="ts">
	import type { SpawnRequest } from '@bindings/SpawnRequest';
	import type { DispatchRequest } from '@bindings/DispatchRequest';
	import type { PermissionMode } from '@bindings/PermissionMode';
	import {
		useAllMachines,
		useDispatchers,
		useSessionActions,
		useRecentDirs,
		useMachineDirs,
		useAccounts
	} from '$lib/queries';
	import { ws } from '$lib/ws.svelte';
	import { toasts } from '$lib/toast.svelte';
	import {
		drafts,
		SPAWN_DRAFT,
		LAST_MACHINE,
		loadMachinePrefs,
		saveMachinePrefs
	} from '$lib/drafts';
	import { autoresize } from '$lib/autoresize';
	import { dropzone } from '$lib/dropzone';
	import {
		MAX_FILE_BYTES,
		MAX_TOTAL_BYTES,
		MAX_FILES,
		mergeFiles,
		removeFileByName,
		fileCapError
	} from '$lib/attachments';
	import BrandLogo from './BrandLogo.svelte';
	import AttachmentList from './AttachmentList.svelte';
	import Modal from './Modal.svelte';

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

	// "machine" = spawn on an enrolled daemon; "dispatch" = hand off to a k8s
	// dispatcher (claude-worker) that runs the session in an ephemeral pod.
	type Target = 'machine' | 'dispatch';
	let target = $state<Target>('machine');

	interface Form {
		machine_id: string;
		adapter_id: string;
		working_dir: string;
		name: string;
		prompt: string;
		permission_mode: PermissionMode;
		// dispatch-only fields (forwarded to the dispatcher as `payload`).
		dispatcher: string;
		identity: string;
		repo: string;
		ticket: string;
		prompt_file: string;
		// Model family is per-adapter (claude families vs codex models), like
		// effort below, so each gets its own field and they survive an adapter
		// switch (CCT-274). Dispatch (k8s) runs a claude worker → model_claude.
		model_claude: string;
		model_codex: string;
		// Named OAuth account to run under (CCT-237), resolved per-adapter at
		// spawn. Empty = no gateway injection (the worker's own auth).
		account: string;
		// Effort is per-adapter (claude and codex have different level sets), so
		// each gets its own slider + form field and they're preserved across an
		// adapter switch. Dispatch (k8s) runs a claude worker → uses effort_claude.
		effort_claude: string;
		effort_codex: string;
		timeout: string;
	}
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
	let form = $state<Form>(load());
	function load(): Form {
		try {
			return { ...blank, ...JSON.parse(drafts.get(SPAWN_DRAFT) || '{}'), ...(prefill ?? {}) };
		} catch {
			return { ...blank, ...(prefill ?? {}) };
		}
	}
	$effect(() => {
		drafts.set(SPAWN_DRAFT, JSON.stringify(form));
	});

	// default the machine to the last used one (else the first) once loaded
	$effect(() => {
		const list = $machines.data ?? [];
		if (form.machine_id || !list.length) return;
		const last = drafts.get(LAST_MACHINE);
		form.machine_id = list.some((m) => m.id === last) ? last : list[0].id;
	});

	// Remember spawn settings PER MACHINE (CCT-274): when the selected machine
	// changes, pull that machine's last-used adapter/model/effort/account so the
	// next spawn on e.g. dev1 re-selects what you usually run there. An explicit
	// prefill (re-dispatch from an existing session) takes precedence — we don't
	// clobber it. We set `prefsLoadedFor` BEFORE writing the fields, so the
	// re-runs triggered by those writes hit the early-return.
	let prefsLoadedFor = $state<string | null>(null);
	$effect(() => {
		const id = form.machine_id;
		if (!id || id === prefsLoadedFor) return;
		prefsLoadedFor = id;
		if (prefill) return;
		const p = loadMachinePrefs(id);
		if (!p) return;
		if (p.adapter_id) form.adapter_id = p.adapter_id;
		if (p.model_claude != null) form.model_claude = p.model_claude;
		if (p.model_codex != null) form.model_codex = p.model_codex;
		if (p.effort_claude != null) form.effort_claude = p.effort_claude;
		if (p.effort_codex != null) form.effort_codex = p.effort_codex;
		if (p.account != null) form.account = p.account;
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

	// --- Working-directory autocomplete ---
	// Split the typed value at the last `/`: list `parent` on the machine,
	// filter by `prefix` locally. Listings are keyed by (machine, parent) so a
	// request only fires when the user crosses a `/` boundary. Dotdirs only
	// surface once the typed segment starts with a dot.
	let cwdFocused = $state(false);
	let cwdHighlight = $state(-1);
	const cwdParent = $derived.by(() => {
		const i = form.working_dir.lastIndexOf('/');
		if (i < 0) return '';
		return i === 0 ? '/' : form.working_dir.slice(0, i);
	});
	const cwdPrefix = $derived.by(() => {
		const i = form.working_dir.lastIndexOf('/');
		return i < 0 ? form.working_dir : form.working_dir.slice(i + 1);
	});
	const machineDirs = useMachineDirs(
		() => (cwdFocused ? form.machine_id : ''),
		() => cwdParent
	);
	const cwdSuggestions = $derived.by(() => {
		const dirs = $machineDirs.data?.dirs ?? [];
		const prefix = cwdPrefix.toLowerCase();
		const showHidden = prefix.startsWith('.');
		return dirs
			.filter((d) => (showHidden || !d.startsWith('.')) && d.toLowerCase().startsWith(prefix))
			.slice(0, 50);
	});
	$effect(() => {
		void cwdSuggestions; // reset highlight whenever the list changes
		cwdHighlight = -1;
	});
	function pickCwd(name: string) {
		form.working_dir = `${cwdParent === '/' ? '' : cwdParent}/${name}/`;
		cwdHighlight = -1;
	}
	function cwdKeydown(e: KeyboardEvent) {
		if (!cwdFocused || !cwdSuggestions.length) return;
		if (e.key === 'ArrowDown') {
			e.preventDefault();
			cwdHighlight = (cwdHighlight + 1) % cwdSuggestions.length;
		} else if (e.key === 'ArrowUp') {
			e.preventDefault();
			cwdHighlight = (cwdHighlight - 1 + cwdSuggestions.length) % cwdSuggestions.length;
		} else if ((e.key === 'Enter' || e.key === 'Tab') && cwdHighlight >= 0) {
			e.preventDefault();
			pickCwd(cwdSuggestions[cwdHighlight]);
		} else if (e.key === 'Tab' && cwdSuggestions.length === 1) {
			e.preventDefault();
			pickCwd(cwdSuggestions[0]);
		} else if (e.key === 'Escape') {
			e.stopPropagation(); // keep the modal open; just dismiss the list
			cwdFocused = false;
		}
	}

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
	let envRows = $state<EnvRow[]>([]);
	let files = $state<File[]>([]);

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
	const addEnvRow = () => (envRows = [...envRows, { key: '', value: '' }]);
	const removeEnvRow = (i: number) => (envRows = envRows.filter((_, idx) => idx !== i));

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
		// Remember these settings for this machine (CCT-274) so the next spawn
		// here pre-selects them. Saved on submit (not just on confirmed success)
		// so a slow/unconfirmed spawn still records the operator's intent.
		saveMachinePrefs(form.machine_id, {
			adapter_id: form.adapter_id,
			model_claude: form.model_claude,
			model_codex: form.model_codex,
			effort_claude: form.effort_claude,
			effort_codex: form.effort_codex,
			account: form.account
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

	const modes: { v: PermissionMode; label: string; hint: string }[] = [
		{ v: 'ask', label: 'Ask', hint: 'Prompt on every action' },
		{ v: 'auto', label: 'Auto', hint: 'Auto-apply, sandbox on' },
		{ v: 'yolo', label: 'Yolo', hint: 'No prompts, full access' }
	];

	// Per-adapter effort levels, index 0 = "" (adapter default). Claude and
	// codex expose different reasoning-effort vocabularies.
	const claudeEfforts = ['', 'low', 'medium', 'high', 'xhigh', 'max'];
	const codexEfforts = ['', 'low', 'medium', 'high', 'xhigh'];

	// Model families per adapter (CCT-274). `''` = the adapter's own default.
	// claude resolves the family alias (opus/sonnet/haiku/fable) to a concrete
	// model; codex takes the model slug directly.
	const claudeModels = [
		{ v: '', label: 'Default' },
		{ v: 'haiku', label: 'Haiku' },
		{ v: 'sonnet', label: 'Sonnet' },
		{ v: 'opus', label: 'Opus' },
		{ v: 'fable', label: 'Fable' }
	];
	const codexModels = [
		{ v: '', label: 'Default' },
		{ v: 'gpt-5.5-codex', label: 'GPT-5.5 Codex' },
		{ v: 'gpt-5.4-codex', label: 'GPT-5.4 Codex' }
	];
</script>

<!-- A discrete effort slider. `levels[0]` is "" (adapter default); the track
     snaps to each named level and the active label is highlighted. -->
{#snippet effortSlider(id: string, levels: string[], current: string, onset: (v: string) => void)}
	{@const idx = Math.max(0, levels.indexOf(current))}
	<div class="field">
		<label class="label" for={id}>Effort</label>
		<input
			{id}
			class="slider"
			type="range"
			min="0"
			max={levels.length - 1}
			step="1"
			value={idx}
			oninput={(e) => onset(levels[Number((e.currentTarget as HTMLInputElement).value)])}
		/>
		<div class="ticks">
			{#each levels as lv, i (lv)}
				<button
					type="button"
					class="tick"
					class:on={i === idx}
					onclick={() => onset(lv)}>{lv || 'default'}</button
				>
			{/each}
		</div>
	</div>
{/snippet}

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
				<div class="field">
					<span class="label">Run on</span>
					<div class="targets">
						<button
							type="button"
							class="target"
							class:sel={target === 'machine'}
							onclick={() => (target = 'machine')}
						>
							<strong>Machine</strong>
							<span class="faint sm">An enrolled daemon</span>
						</button>
						<button
							type="button"
							class="target"
							class:sel={target === 'dispatch'}
							onclick={() => (target = 'dispatch')}
						>
							<strong>Dispatch (k8s)</strong>
							<span class="faint sm">Ephemeral worker pod</span>
						</button>
					</div>
				</div>
			{/if}

			{#if target === 'dispatch'}
				{#if dispatcherIds.length > 1}
					<div class="field">
						<label class="label" for="sp-dispatcher">Dispatcher</label>
						<select id="sp-dispatcher" class="select" bind:value={form.dispatcher}>
							{#each dispatcherIds as d (d)}<option value={d}>{d}</option>{/each}
						</select>
					</div>
				{/if}

				<div class="field">
					<label class="label" for="sp-name-d">Name (optional)</label>
					<input
						id="sp-name-d"
						class="input"
						placeholder="session label"
						bind:value={form.name}
					/>
					<span class="faint sm">Passed to the worker as <code>--name</code>.</span>
				</div>

				<div class="field">
					<label class="label" for="sp-identity">Identity (optional)</label>
					<input
						id="sp-identity"
						class="input mono"
						placeholder="alice"
						bind:value={form.identity}
					/>
					<span class="faint sm">Which account the worker acts as. Empty = worker default.</span>
				</div>

				<div class="field">
					<label class="label" for="sp-repo">Repo</label>
					<input
						id="sp-repo"
						class="input mono"
						placeholder="cctui"
						bind:value={form.repo}
					/>
					<span class="faint sm">Checked out under the worker's /workspace (optional).</span>
				</div>

				<div class="field">
					<label class="label" for="sp-ticket">Ticket (optional)</label>
					<input
						id="sp-ticket"
						class="input mono"
						placeholder="PROJ-1234"
						bind:value={form.ticket}
					/>
					<span class="faint sm">Issue id for the flow's context (e.g. an implement prompt).</span>
				</div>

				<div class="field">
					<label class="label" for="sp-prompt-d">Prompt</label>
					<textarea
						id="sp-prompt-d"
						class="textarea prompt"
						placeholder="What should the worker do? (e.g. work on CCT-123 / a PR)"
						bind:value={form.prompt}
						use:autoresize={form.prompt}
					></textarea>
				</div>

				<div class="field">
					<label class="label" for="sp-prompt-file">Prompt file (optional)</label>
					<input
						id="sp-prompt-file"
						class="input mono"
						placeholder="implement-from-ticket.md"
						bind:value={form.prompt_file}
					/>
					<span class="faint sm">A file under the worker's /prompts. Overrides the inline prompt.</span>
				</div>

				<div class="row gap">
					<div class="field grow">
						<label class="label" for="sp-model">Model</label>
						<!-- Dispatch runs a claude worker → claude families. -->
						<select id="sp-model" class="select" bind:value={form.model_claude}>
							{#each claudeModels as m (m.v)}<option value={m.v}>{m.label}</option>{/each}
						</select>
					</div>
					<div class="field grow">
						<label class="label" for="sp-timeout">Timeout min</label>
						<input
							id="sp-timeout"
							class="input mono"
							inputmode="numeric"
							placeholder="default"
							bind:value={form.timeout}
						/>
					</div>
				</div>

				<!-- Dispatch runs a claude worker → claude effort levels. -->
				{@render effortSlider(
					'sp-effort-d',
					claudeEfforts,
					form.effort_claude,
					(v) => (form.effort_claude = v)
				)}
			{:else}
			<div class="field">
				<label class="label" for="sp-machine">Machine</label>
				<select id="sp-machine" class="select" bind:value={form.machine_id}>
					{#if !($machines.data ?? []).length}
						<option value="">No machines enrolled</option>
					{/if}
					{#each $machines.data ?? [] as mc (mc.id)}
						<option value={mc.id}>{mc.display_name || mc.name}</option>
					{/each}
				</select>
			</div>

			<div class="field">
				<label class="label" for="sp-cwd">Working directory</label>
				{#if recentDirs.length}
					<select
						class="select mono"
						aria-label="Recent directories"
						value={recentDirs.includes(form.working_dir) ? form.working_dir : ''}
						onchange={(e) => (form.working_dir = (e.currentTarget as HTMLSelectElement).value)}
					>
						<option value="">Recent directories…</option>
						{#each recentDirs as d (d)}<option value={d}>{d}</option>{/each}
					</select>
				{/if}
				<div class="cwd-combo">
					<input
						id="sp-cwd"
						class="input mono"
						placeholder="/home/user/project"
						autocomplete="off"
						bind:value={form.working_dir}
						onfocus={() => (cwdFocused = true)}
						oninput={() => (cwdFocused = true)}
						onblur={() => setTimeout(() => (cwdFocused = false), 150)}
						onkeydown={cwdKeydown}
					/>
					{#if cwdFocused && cwdSuggestions.length}
						<ul class="cwd-suggestions" role="listbox" aria-label="Directory suggestions">
							{#each cwdSuggestions as d, i (d)}
								<!-- svelte-ignore a11y_click_events_have_key_events -->
								<li
									role="option"
									aria-selected={i === cwdHighlight}
									class:active={i === cwdHighlight}
									onmousedown={(e) => {
										e.preventDefault();
										pickCwd(d);
									}}
								>
									{d}/
								</li>
							{/each}
						</ul>
					{/if}
				</div>
			</div>

			<div class="field">
				<label class="label" for="sp-name">Name (optional)</label>
				<input id="sp-name" class="input" placeholder="session label" bind:value={form.name} />
			</div>

			<div class="field">
				<label class="label" for="sp-prompt">Prompt (optional)</label>
				<textarea
					id="sp-prompt"
					class="textarea prompt"
					placeholder="Initial prompt…"
					bind:value={form.prompt}
					use:autoresize={form.prompt}
				></textarea>
			</div>

			<div class="field">
				<span class="label">Adapter</span>
				<div class="adapters">
					<button
						type="button"
						class="adapter-opt claude"
						class:sel={form.adapter_id === 'claude-code'}
						onclick={() => (form.adapter_id = 'claude-code')}
					>
						<BrandLogo adapter="claude-code" size={18} />
						<span>Claude Code</span>
					</button>
					<button
						type="button"
						class="adapter-opt codex"
						class:sel={form.adapter_id === 'codex'}
						onclick={() => (form.adapter_id = 'codex')}
					>
						<BrandLogo adapter="codex" size={18} />
						<span>Codex</span>
					</button>
				</div>
			</div>

			<!-- Model family (CCT-274), per adapter. `Default` = the adapter's own
			     default model. -->
			<div class="field">
				<label class="label" for="sp-model-m">Model</label>
				{#if form.adapter_id === 'codex'}
					<select id="sp-model-m" class="select" bind:value={form.model_codex}>
						{#each codexModels as m (m.v)}<option value={m.v}>{m.label}</option>{/each}
					</select>
				{:else}
					<select id="sp-model-m" class="select" bind:value={form.model_claude}>
						{#each claudeModels as m (m.v)}<option value={m.v}>{m.label}</option>{/each}
					</select>
				{/if}
			</div>

			<!-- OAuth account picker (CCT-237). Only accounts whose provider
			     matches the selected adapter are offered; empty = the worker's
			     own auth (no gateway). -->
			{#if matchingAccounts.length}
				<div class="field">
					<label class="label" for="sp-account">Account (optional)</label>
					<select id="sp-account" class="select" bind:value={form.account}>
						<option value="">Default (worker's own auth)</option>
						{#each matchingAccounts as a (a.id)}
							<option value={a.name}>{a.name}</option>
						{/each}
					</select>
					<span class="faint sm">Run through the passthrough gateway under this account.</span>
				</div>
			{/if}

			<!-- Per-adapter effort: each adapter has its own level set and its own
			     form field, so switching adapters preserves both. -->
			{#if form.adapter_id === 'codex'}
				{@render effortSlider(
					'sp-effort-codex',
					codexEfforts,
					form.effort_codex,
					(v) => (form.effort_codex = v)
				)}
			{:else}
				{@render effortSlider(
					'sp-effort-claude',
					claudeEfforts,
					form.effort_claude,
					(v) => (form.effort_claude = v)
				)}
			{/if}

			<div class="field">
				<span class="label">Permission mode</span>
				<div class="modes">
					{#each modes as md (md.v)}
						<button
							type="button"
							class="mode mode-{md.v}"
							class:sel={form.permission_mode === md.v}
							onclick={() => (form.permission_mode = md.v)}
						>
							<strong>{md.label}</strong>
							<span class="faint sm">{md.hint}</span>
						</button>
					{/each}
				</div>
			</div>

			<!-- File uploads (CCT-203), machine target only. Staged under
			     /tmp/cctui-uploads/<session>/ on the daemon and referenced in
			     the prompt so the worker can read them. -->
			<div class="field">
				<span class="label">Attachments (optional)</span>
				<input
					id="sp-files"
					class="file"
					type="file"
					multiple
					onchange={onPickFiles}
				/>
				<span class="faint sm">
					Up to {MAX_FILES} files, {MAX_FILE_BYTES / 1024 / 1024} MB each /
					{MAX_TOTAL_BYTES / 1024 / 1024} MB total. Drag-and-drop anywhere on this
					dialog or pick. Staged on the machine and referenced in the prompt.
				</span>
				<AttachmentList {files} onremove={removeFile} />
			</div>
			{/if}

			<!-- Environment secrets (CCT-202), both targets. Values are injected as
			     env vars in the worker process, never shown in the conversation,
			     and fixed for the session's lifetime. -->
			<div class="field">
				<span class="label">Environment secrets</span>
				<span class="faint sm">
					Injected as env vars in the worker — not visible in the conversation,
					logs, or transcript, and fixed for the session's lifetime.
				</span>
				{#each envRows as row, i (i)}
					<div class="row gap">
						<input
							class="input mono grow"
							placeholder="API_KEY"
							aria-label="Secret name"
							bind:value={row.key}
						/>
						<input
							class="input mono grow"
							type="password"
							placeholder="value"
							aria-label="Secret value"
							bind:value={row.value}
						/>
						<button type="button" class="x" onclick={() => removeEnvRow(i)}>✕</button>
					</div>
				{/each}
				<button type="button" class="btn-control add-secret" onclick={addEnvRow}>+ Add secret</button>
				{#if badEnvKeys.length}
					<span class="err sm">Keys must match <code>^[A-Z_][A-Z0-9_]*$</code></span>
				{/if}
			</div>
		</div>
	{/snippet}
	{#snippet footer()}
		<button class="btn" onclick={clearForm}>Clear</button>
		<button class="btn btn-primary btn-block" disabled={busy || !valid} onclick={submit}>
			{#if busy}<span class="spin"></span>{:else}{target === 'machine' ? 'Spawn' : 'Dispatch'}{/if}
		</button>
	{/snippet}
</Modal>

<style>
	.sm {
		font-size: var(--fs-xs);
	}
	.modes {
		display: grid;
		grid-template-columns: 1fr 1fr 1fr;
		gap: var(--sp-2);
	}
	.targets {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: var(--sp-2);
	}
	.target {
		display: flex;
		flex-direction: column;
		gap: 2px;
		padding: var(--sp-2);
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		text-align: left;
	}
	.target.sel {
		border-color: var(--c-blue);
		background: color-mix(in srgb, var(--c-blue) 14%, var(--bg));
		color: var(--c-blue);
	}
	.target.sel .faint {
		color: color-mix(in srgb, var(--c-blue) 70%, var(--text-muted));
	}
	.row.gap {
		display: flex;
		gap: var(--sp-2);
	}
	.slider {
		width: 100%;
		accent-color: var(--c-blue);
		margin: 2px 0;
	}
	.ticks {
		display: flex;
		justify-content: space-between;
		gap: var(--sp-1);
	}
	.tick {
		flex: 1;
		padding: 2px 0;
		background: none;
		border: none;
		text-align: center;
		font-size: var(--fs-xs);
		color: var(--text-muted);
		cursor: pointer;
	}
	.tick.on {
		color: var(--c-blue);
		font-weight: var(--fw-medium);
	}
	.grow {
		flex: 1;
		min-width: 0;
	}
	.file {
		font-size: var(--fs-xs);
		color: var(--text-muted);
	}
	/* A real, thumb-sized browse button (CCT-241) — the UA default is tiny,
	   which makes the picker nearly untappable on mobile. */
	.file::file-selector-button {
		padding: var(--sp-2) var(--sp-4);
		margin-right: var(--sp-3);
		font-size: var(--fs-sm);
		font-weight: var(--fw-medium);
		color: var(--text);
		background: var(--bg-elevated-2);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		cursor: pointer;
	}
	.file::file-selector-button:hover {
		border-color: var(--c-blue);
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
	.x {
		background: none;
		border: none;
		color: var(--text-muted);
		cursor: pointer;
		padding: 0 var(--sp-1);
		font-size: var(--fs-sm);
	}
	.x:hover {
		color: var(--c-red);
	}
	.btn-sm {
		align-self: flex-start;
		font-size: var(--fs-xs);
		padding: 2px var(--sp-2);
	}
	/* Add-secret button shares the shared control sizing/shape with the
	   "browse files to upload" button (CCT-250 item 1 + 7). */
	.add-secret {
		align-self: flex-start;
	}
	.err {
		color: var(--c-red);
	}
	.mode {
		display: flex;
		flex-direction: column;
		gap: 2px;
		padding: var(--sp-2);
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		text-align: left;
	}
	.adapters {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: var(--sp-2);
	}
	.adapter-opt {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: var(--sp-2);
		padding: var(--sp-2);
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		color: var(--text-muted);
		font-weight: var(--fw-medium);
	}
	.adapter-opt.claude {
		--brand: var(--c-amber);
	}
	.adapter-opt.codex {
		--brand: var(--c-blue);
	}
	.adapter-opt.sel {
		border-color: var(--brand);
		background: color-mix(in srgb, var(--brand) 14%, var(--bg));
		color: var(--brand);
	}
	.prompt {
		min-height: 8rem;
		max-height: 60vh;
		resize: none;
		overflow-y: auto;
	}
	/* Per-mode accent: ask = green (safe), auto = blue (sandboxed),
	   yolo = red (no prompts, full access). */
	.mode-ask {
		--mode-c: var(--c-green);
	}
	.mode-auto {
		--mode-c: var(--c-blue);
	}
	.mode-yolo {
		--mode-c: var(--c-red);
	}
	.mode.sel {
		border-color: var(--mode-c);
		background: color-mix(in srgb, var(--mode-c) 16%, var(--bg));
		color: var(--mode-c);
	}
	.mode.sel .faint {
		color: color-mix(in srgb, var(--mode-c) 70%, var(--text-muted));
	}
	.cwd-combo {
		position: relative;
	}
	.cwd-suggestions {
		position: absolute;
		top: 100%;
		left: 0;
		right: 0;
		z-index: 10;
		margin: 2px 0 0;
		padding: var(--sp-1);
		list-style: none;
		max-height: 220px;
		overflow-y: auto;
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		font-size: var(--fs-xs);
	}
	.cwd-suggestions li {
		padding: 2px var(--sp-2);
		border-radius: var(--r-sm);
		cursor: pointer;
	}
	.cwd-suggestions li:hover,
	.cwd-suggestions li.active {
		background: color-mix(in srgb, var(--c-blue) 14%, var(--bg));
		color: var(--c-blue);
	}
</style>
