<script lang="ts">
	// The "Machine" branch of the spawn form, extracted from SpawnModal: machine
	// select, working directory (CwdCombo), name, prompt, then the Account → Model
	// → Harness axis (CCT-399), effort, permission mode. Spawns on an enrolled
	// daemon. File attachments live in the parent's shared add-ons row.
	//
	// Account is the primary axis (CCT-399): choosing an account locks the harness
	// to whatever it's compatible with and drives the model list from the
	// account's declared models (compatible endpoints) or native families
	// (subscription). "Default (no account)" preserves the old adapter-first flow.
	import type { MachineRow } from '@bindings/MachineRow';
	import type { OAuthAccount } from '$lib/queries';
	import BrandLogo from '$lib/components/atoms/BrandLogo.svelte';
	import { Field, Input, OptionButton, Select, Text, Textarea } from '@dorsk/tsumikit';
	import CwdCombo from './CwdCombo.svelte';
	import EffortSlider from './EffortSlider.svelte';
	import {
		claudeModels,
		codexModels,
		claudeEfforts,
		codexEfforts,
		modes,
		adapterForProvider,
		isCompatibleProvider,
		adapterLabel
	} from './options';
	import { submitChordLabel, isSubmitChord } from '$lib/platform';
	import type { Form } from './types';

	let {
		form = $bindable(),
		machines,
		recentDirs,
		accounts,
		onsubmit
	}: {
		form: Form;
		machines: MachineRow[];
		recentDirs: string[];
		// Every account the caller owns (CCT-399). The picker offers them all and
		// derives the harness + model list from the chosen one.
		accounts: OAuthAccount[];
		// Submit the whole spawn form from the prompt textarea (Ctrl/⌘+Enter).
		onsubmit?: () => void;
	} = $props();

	// The account currently selected (matched on name + provider so a name shared
	// across providers stays unambiguous). Empty form.account = "Default".
	const selectedAccount = $derived(
		form.account
			? (accounts.find((a) => a.name === form.account && a.provider === form.account_provider) ??
					accounts.find((a) => a.name === form.account))
			: undefined
	);

	// The effective harness: locked to the account's family when one is chosen,
	// else the user-picked adapter ("Default" flow).
	const effectiveAdapter = $derived(
		selectedAccount ? adapterForProvider(selectedAccount.provider) : form.adapter_id
	);

	// Model options for the chosen axis (CCT-399):
	//  * compatible account → its own declared models;
	//  * native account / Default → the harness's native families.
	const accountModelOptions = $derived(
		(selectedAccount?.models ?? []).map((m) => ({ v: m.model, label: m.label }))
	);
	const usesAccountModels = $derived(
		!!selectedAccount && isCompatibleProvider(selectedAccount.provider)
	);

	// Clear a stale account selection if it no longer exists (e.g. accounts
	// reloaded), and keep account_provider in step with the chosen account.
	$effect(() => {
		if (form.account && !accounts.some((a) => a.name === form.account)) {
			form.account = '';
			form.account_provider = '';
		}
	});

	// Per-mode accent: ask = green (safe), auto = blue (sandboxed),
	// yolo = red (no prompts, full access).
	const modeAccent: Record<string, string> = {
		ask: 'var(--c-green)',
		auto: 'var(--c-blue)',
		yolo: 'var(--c-red)',
		whip: 'var(--c-red)'
	};

	function onAccountChange(value: string) {
		form.account = value;
		form.account_provider = value
			? (accounts.find((a) => a.name === value)?.provider ?? '')
			: '';
	}
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

<!-- Account → Model → Harness (CCT-399). The account is the primary axis: it
     locks the harness and drives the model list. "Default (no account)" keeps
     the old adapter-first flow below. -->
<Field label="Account" for="sp-account">
	<Select id="sp-account" value={form.account} onchange={(e) => onAccountChange((e.currentTarget as HTMLSelectElement).value)}>
		<option value="">Default (no account)</option>
		{#each accounts as a (a.id)}
			<option value={a.name}>{a.name} ({a.provider})</option>
		{/each}
	</Select>
	{#if selectedAccount}
		<Text tone="faint" size="xs">Runs through the passthrough gateway under this account.</Text>
	{:else}
		<Text tone="faint" size="xs">Default uses the worker's own auth.</Text>
	{/if}
</Field>

{#if selectedAccount}
	<!-- Model: driven by the account (compatible → its models; native → families). -->
	<Field label="Model" for="sp-model-acct">
		{#if usesAccountModels}
			<Select id="sp-model-acct" bind:value={form.model_account}>
				{#if !accountModelOptions.length}
					<option value="">Default</option>
				{/if}
				{#each accountModelOptions as m (m.v)}<option value={m.v}>{m.label}</option>{/each}
			</Select>
		{:else if effectiveAdapter === 'codex'}
			<Select id="sp-model-acct" bind:value={form.model_codex}>
				{#each codexModels as m (m.v)}<option value={m.v}>{m.label}</option>{/each}
			</Select>
		{:else}
			<Select id="sp-model-acct" bind:value={form.model_claude}>
				{#each claudeModels as m (m.v)}<option value={m.v}>{m.label}</option>{/each}
			</Select>
		{/if}
	</Field>

	<!-- Harness: locked + read-only when an account is chosen. -->
	<Field label="Harness">
		<div class="locked-harness">
			<BrandLogo adapter={effectiveAdapter} size={18} />
			<Text>{adapterLabel(effectiveAdapter)}</Text>
			<Text tone="faint" size="xs">locked by account</Text>
		</div>
	</Field>
{:else}
	<!-- Default (no account): adapter-first flow. -->
	<Field label="Harness">
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
{/if}

<!-- Per-adapter effort: keyed off the effective harness so an account-locked
     codex still shows codex efforts. -->
{#if effectiveAdapter === 'codex'}
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
	.locked-harness {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		background: var(--bg-elevated-2);
	}
	.modes {
		display: grid;
		grid-template-columns: 1fr 1fr 1fr;
		gap: var(--sp-2);
	}
</style>
