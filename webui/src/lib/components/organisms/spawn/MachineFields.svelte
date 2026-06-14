<script lang="ts">
	// The "Machine" branch of the spawn form, extracted from SpawnModal: machine
	// select, working directory (CwdCombo), name, prompt, adapter picker, model,
	// OAuth account, effort, permission mode. Spawns on an enrolled daemon. File
	// attachments live in the parent's shared add-ons row (both targets share it).
	import type { MachineRow } from '@bindings/MachineRow';
	import type { OAuthAccount } from '$lib/queries';
	import BrandLogo from '$lib/components/atoms/BrandLogo.svelte';
	import { Field, Input, OptionButton, Select, Text, Textarea } from '@dorsk/tsumikit';
	import CwdCombo from './CwdCombo.svelte';
	import EffortSlider from './EffortSlider.svelte';
	import { claudeModels, codexModels, claudeEfforts, codexEfforts, modes } from './options';
	import { submitChordLabel, isSubmitChord } from '$lib/platform';
	import type { Form } from './types';

	let {
		form = $bindable(),
		machines,
		recentDirs,
		matchingAccounts,
		onsubmit
	}: {
		form: Form;
		machines: MachineRow[];
		recentDirs: string[];
		matchingAccounts: OAuthAccount[];
		// Submit the whole spawn form from the prompt textarea (Ctrl/⌘+Enter).
		onsubmit?: () => void;
	} = $props();

	// Per-mode accent: ask = green (safe), auto = blue (sandboxed),
	// yolo = red (no prompts, full access).
	const modeAccent: Record<string, string> = {
		ask: 'var(--c-green)',
		auto: 'var(--c-blue)',
		yolo: 'var(--c-red)'
	};
</script>

<Field label="Machine" for="sp-machine">
	<Select id="sp-machine" bind:value={form.machine_id}>
		{#if !machines.length}
			<option value="">No machines enrolled</option>
		{/if}
		{#each machines as mc (mc.id)}
			<option value={mc.id}>{mc.display_name || mc.name}</option>
		{/each}
	</Select>
</Field>

<CwdCombo machineId={form.machine_id} bind:value={form.working_dir} {recentDirs} />

<Field label="Name (optional)" for="sp-name">
	<Input id="sp-name" placeholder="session label" bind:value={form.name} />
</Field>

<Field label="Prompt (optional)" for="sp-prompt">
	<Textarea
		id="sp-prompt"
		style="min-height:8rem;max-height:60vh;resize:none;overflow-y:auto"
		placeholder="Initial prompt…"
		bind:value={form.prompt}
		autoresize
		onkeydown={(e: KeyboardEvent) => {
			if (onsubmit && isSubmitChord(e)) {
				e.preventDefault();
				onsubmit();
			}
		}}
	/>
	<Text size="xs" tone="faint" style="display:block;margin-top:var(--sp-1)">{submitChordLabel()} to create</Text>
</Field>

<Field label="Adapter">
	<div class="adapters">
		<OptionButton
			row
			selected={form.adapter_id === 'claude-code'}
			style="--opt-accent: var(--c-amber)"
			onclick={() => (form.adapter_id = 'claude-code')}
		>
			<BrandLogo adapter="claude-code" size={18} />
			<Text>Claude Code</Text>
		</OptionButton>
		<OptionButton
			row
			selected={form.adapter_id === 'codex'}
			style="--opt-accent: var(--c-blue)"
			onclick={() => (form.adapter_id = 'codex')}
		>
			<BrandLogo adapter="codex" size={18} />
			<Text>Codex</Text>
		</OptionButton>
	</div>
</Field>

<!-- Model family (CCT-274), per adapter. `Default` = the adapter's own
     default model. -->
<Field label="Model" for="sp-model-m">
	{#if form.adapter_id === 'codex'}
		<Select id="sp-model-m" bind:value={form.model_codex}>
			{#each codexModels as m (m.v)}<option value={m.v}>{m.label}</option>{/each}
		</Select>
	{:else}
		<Select id="sp-model-m" bind:value={form.model_claude}>
			{#each claudeModels as m (m.v)}<option value={m.v}>{m.label}</option>{/each}
		</Select>
	{/if}
</Field>

<!-- OAuth account picker (CCT-237). Only accounts whose provider matches the
     selected adapter are offered; empty = the worker's own auth (no gateway). -->
{#if matchingAccounts.length}
	<Field label="Account (optional)" for="sp-account">
		<Select id="sp-account" bind:value={form.account}>
			<option value="">Default (worker's own auth)</option>
			{#each matchingAccounts as a (a.id)}
				<option value={a.name}>{a.name}</option>
			{/each}
		</Select>
		<Text tone="faint" size="xs">Run through the passthrough gateway under this account.</Text>
	</Field>
{/if}

<!-- Per-adapter effort: each adapter has its own level set and its own form
     field, so switching adapters preserves both. -->
{#if form.adapter_id === 'codex'}
	<EffortSlider
		id="sp-effort-codex"
		levels={codexEfforts}
		current={form.effort_codex}
		onset={(v) => (form.effort_codex = v)}
	/>
{:else}
	<EffortSlider
		id="sp-effort-claude"
		levels={claudeEfforts}
		current={form.effort_claude}
		onset={(v) => (form.effort_claude = v)}
	/>
{/if}

<Field label="Permission mode">
	<div class="modes">
		{#each modes as md (md.v)}
			<OptionButton
				selected={form.permission_mode === md.v}
				style={`--opt-accent: ${modeAccent[md.v]}`}
				onclick={() => (form.permission_mode = md.v)}
			>
				<strong>{md.label}</strong>
				<Text tone="faint" size="xs">{md.hint}</Text>
			</OptionButton>
		{/each}
	</div>
</Field>

<style>
	.adapters {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: var(--sp-2);
	}
	.modes {
		display: grid;
		grid-template-columns: 1fr 1fr 1fr;
		gap: var(--sp-2);
	}
</style>
