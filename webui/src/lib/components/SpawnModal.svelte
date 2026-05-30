<script lang="ts">
	import type { SpawnRequest } from '@bindings/SpawnRequest';
	import type { PermissionMode } from '@bindings/PermissionMode';
	import { useAllMachines, useSessionActions, useRecentDirs } from '$lib/queries';
	import { ws } from '$lib/ws.svelte';
	import { toasts } from '$lib/toast.svelte';
	import { drafts, SPAWN_DRAFT, LAST_MACHINE } from '$lib/drafts';
	import { autoresize } from '$lib/autoresize';
	import BrandLogo from './BrandLogo.svelte';
	import Modal from './Modal.svelte';

	let { onclose, onspawned }: { onclose: () => void; onspawned: () => void } = $props();

	const machines = useAllMachines(() => true);

	interface Form {
		machine_id: string;
		adapter_id: string;
		working_dir: string;
		name: string;
		prompt: string;
		permission_mode: PermissionMode;
	}
	const blank: Form = {
		machine_id: '',
		adapter_id: 'claude-code',
		working_dir: '',
		name: '',
		prompt: '',
		permission_mode: 'yolo'
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

	// recent working dirs on the selected machine, from the server (last 5).
	const dirsQuery = useRecentDirs(() => form.machine_id);
	const recentDirs = $derived([...new Set($dirsQuery.data ?? [])]);

	const actions = useSessionActions();
	let busy = $state(false);

	async function submit() {
		if (!form.machine_id || !form.working_dir.trim() || busy) return;
		busy = true;
		const body: SpawnRequest = {
			machine_id: form.machine_id,
			working_dir: form.working_dir.trim(),
			adapter_id: form.adapter_id,
			name: form.name.trim() || null,
			prompt: form.prompt.trim() || null,
			prompt_name: null,
			permission_mode: form.permission_mode
		};
		try {
			const res = await actions.spawn(body);
			drafts.set(LAST_MACHINE, form.machine_id);
			toasts.push('Spawning…', 'info');
			const result = await ws.awaitCommand(res.command_id);
			if (result.ok) {
				toasts.ok('Session spawned');
				drafts.clear(SPAWN_DRAFT);
				form = { ...blank, machine_id: form.machine_id };
				onspawned();
				onclose();
			} else {
				toasts.err(`Spawn failed: ${result.error ?? 'unknown error'}`);
			}
		} catch (e) {
			toasts.err(`Spawn failed: ${(e as Error).message}`);
		} finally {
			busy = false;
		}
	}

	function clearForm() {
		form = { ...blank, machine_id: form.machine_id };
		drafts.clear(SPAWN_DRAFT);
	}

	const modes: { v: PermissionMode; label: string; hint: string }[] = [
		{ v: 'ask', label: 'Ask', hint: 'Prompt on every action' },
		{ v: 'auto', label: 'Auto', hint: 'Auto-apply, sandbox on' },
		{ v: 'yolo', label: 'Yolo', hint: 'No prompts, full access' }
	];
</script>

<Modal title="New session" {onclose}>
	{#snippet body()}
		<div class="stack">
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
		</div>
	{/snippet}
	{#snippet footer()}
		<button class="btn" onclick={clearForm}>Clear</button>
		<button
			class="btn btn-primary btn-block"
			disabled={busy || !form.machine_id || !form.working_dir.trim()}
			onclick={submit}
		>
			{#if busy}<span class="spin"></span>{:else}Spawn{/if}
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
