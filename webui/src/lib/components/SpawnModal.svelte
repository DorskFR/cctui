<script lang="ts">
	import type { SpawnRequest } from '@bindings/SpawnRequest';
	import type { DispatchRequest } from '@bindings/DispatchRequest';
	import type { PermissionMode } from '@bindings/PermissionMode';
	import { useAllMachines, useDispatchers, useSessionActions, useRecentDirs } from '$lib/queries';
	import { ws } from '$lib/ws.svelte';
	import { toasts } from '$lib/toast.svelte';
	import { drafts, SPAWN_DRAFT, LAST_MACHINE } from '$lib/drafts';
	import { autoresize } from '$lib/autoresize';
	import BrandLogo from './BrandLogo.svelte';
	import Modal from './Modal.svelte';

	let { onclose, onspawned }: { onclose: () => void; onspawned: () => void } = $props();

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
		repo: string;
		prompt_file: string;
		model: string;
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
		repo: '',
		prompt_file: '',
		model: '',
		effort_claude: '',
		effort_codex: '',
		timeout: ''
	};
	let form = $state<Form>(load());
	function load(): Form {
		try {
			return { ...blank, ...JSON.parse(drafts.get(SPAWN_DRAFT) || '{}') };
		} catch {
			return { ...blank };
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

	const actions = useSessionActions();
	let busy = $state(false);

	const spawnValid = $derived(!!form.machine_id && !!form.working_dir.trim());
	// A dispatched worker needs a dispatcher and something to run (inline prompt
	// or a server-side prompt file). The repo is optional (the worker falls back
	// to its default cwd), but in practice you'll want one.
	const dispatchValid = $derived(
		!!form.dispatcher && (!!form.prompt.trim() || !!form.prompt_file.trim())
	);
	const valid = $derived(target === 'machine' ? spawnValid : dispatchValid);

	async function spawnOnMachine() {
		const body: SpawnRequest = {
			machine_id: form.machine_id,
			working_dir: form.working_dir.trim(),
			adapter_id: form.adapter_id,
			name: form.name.trim() || null,
			prompt: form.prompt.trim() || null,
			prompt_name: null,
			permission_mode: form.permission_mode,
			effort: (form.adapter_id === 'codex' ? form.effort_codex : form.effort_claude) || null
		};
		const res = await actions.spawn(body);
		drafts.set(LAST_MACHINE, form.machine_id);
		toasts.push('Spawning…', 'info');
		const result = await ws.awaitCommand(res.command_id);
		if (result.ok) {
			toasts.ok('Session spawned');
			drafts.clear(SPAWN_DRAFT);
			form = { ...blank, machine_id: form.machine_id, dispatcher: form.dispatcher };
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
		const payload: Record<string, string> = {};
		if (form.name.trim()) payload.name = form.name.trim();
		if (form.repo.trim()) payload.repo = form.repo.trim();
		if (form.prompt.trim()) payload.prompt = form.prompt.trim();
		if (form.prompt_file.trim()) payload.prompt_file = form.prompt_file.trim();
		if (form.model.trim()) payload.model = form.model.trim();
		if (form.effort_claude.trim()) payload.effort = form.effort_claude.trim();
		const timeout = form.timeout.trim() ? Number(form.timeout.trim()) : null;
		// Client-minted id doubles as the idempotency key (CCT-107); held stable
		// across retries (CCT-193) so a re-submit dedups to the same session.
		pendingDispatchId ??= crypto.randomUUID();
		const body: DispatchRequest = {
			dispatcher: form.dispatcher,
			session_id: pendingDispatchId,
			timeout: Number.isFinite(timeout) ? timeout : null,
			reply_url: null,
			payload
		};
		const res = await actions.dispatch(body);
		toasts.ok(`Dispatched to ${res.dispatcher} (${res.handle})`);
		pendingDispatchId = null;
		drafts.clear(SPAWN_DRAFT);
		form = { ...blank, machine_id: form.machine_id, dispatcher: form.dispatcher };
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
	const codexEfforts = ['', 'minimal', 'low', 'medium', 'high'];
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

<Modal title="New session" {onclose}>
	{#snippet body()}
		<div class="stack">
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
						<label class="label" for="sp-model">Model (optional)</label>
						<input id="sp-model" class="input mono" placeholder="opus" bind:value={form.model} />
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
				<input
					id="sp-cwd"
					class="input mono"
					placeholder="/home/user/project"
					bind:value={form.working_dir}
				/>
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
			{/if}
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
</style>
