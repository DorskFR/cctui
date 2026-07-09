<script lang="ts">
	// The "Machine" branch of the spawn form (CCT-562): a machine · working-dir ·
	// label row, the prompt, then a collapsed configuration line (account ·
	// harness · model · effort · permission mode) that a gear expands into the
	// full editors for this session only.
	//
	// Account is the primary axis (CCT-399), but no longer a hard lock: the
	// harness cards are enabled per the selected account's provider-family union
	// (CCT-562) — an account with anthropic+openai providers offers both, a
	// single-provider account disables the missing card. The provider credential
	// backing the effective harness drives the model list + aliases.
	import type { MachineRow } from '@bindings/MachineRow';
	import type { OAuthAccount } from '$lib/queries';
	import BrandLogo from '$lib/components/atoms/BrandLogo.svelte';
	import { Field, IconButton, Input, OptionButton, Select, Text, Textarea } from '@dorsk/tsumikit';
	import CwdCombo from './CwdCombo.svelte';
	import EffortSlider from './EffortSlider.svelte';
	import {
		claudeModels,
		codexModels,
		claudeEfforts,
		codexEfforts,
		modes,
		allAdapters,
		accountAdapters,
		providerForAdapter,
		effectiveAdapterFor,
		isCompatibleProvider,
		withAliasTargets,
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
		// derives the allowed harnesses + model list from the chosen one.
		accounts: OAuthAccount[];
		// Submit the whole spawn form from the prompt textarea (Ctrl/⌘+Enter).
		onsubmit?: () => void;
	} = $props();

	// Accounts are identities (CCT-558): matched by name. Empty = "Default".
	const selectedAccount = $derived(
		form.account ? accounts.find((a) => a.name === form.account) : undefined
	);

	// Harnesses the selected account can run (provider-family union, CCT-562);
	// no account = both. The effective harness is the user's pick when allowed,
	// else the account's first family.
	const allowedAdapters = $derived(selectedAccount ? accountAdapters(selectedAccount) : allAdapters);
	const effectiveAdapter = $derived(effectiveAdapterFor(selectedAccount, form.adapter_id));
	// The provider credential backing the effective harness on this account.
	const selectedProvider = $derived(providerForAdapter(selectedAccount, effectiveAdapter));

	// Model options for the chosen axis (CCT-399):
	//  * compatible account → its own declared models;
	//  * native account / Default → the harness's native families.
	const accountModelOptions = $derived(
		(selectedProvider?.models ?? []).map((m) => ({ v: m.model, label: m.label }))
	);
	const usesAccountModels = $derived(
		!!selectedProvider && isCompatibleProvider(selectedProvider.provider)
	);
	// Native claude families annotated with the selected account's alias targets.
	const claudeModelOptions = $derived(
		withAliasTargets(claudeModels, selectedProvider?.model_aliases)
	);

	// Clear a stale account selection if it no longer exists (e.g. accounts
	// reloaded); keep account_provider tracking the credential actually in use so
	// the spawn request stays unambiguous.
	$effect(() => {
		if (form.account && !accounts.some((a) => a.name === form.account)) {
			form.account = '';
		}
	});
	$effect(() => {
		form.account_provider = selectedProvider?.provider ?? '';
	});

	// Collapsed configuration line (CCT-562): one summary of the five knobs;
	// expansion is per-open only (not persisted). The knobs prefill from the
	// (machine, cwd) spawn memory (CCT-561, SpawnModal's memory effects).
	let configOpen = $state(false);
	const summaryModel = $derived(
		usesAccountModels
			? form.model_account
			: effectiveAdapter === 'codex'
				? form.model_codex
				: form.model_claude
	);
	const summaryEffort = $derived(
		effectiveAdapter === 'codex' ? form.effort_codex : form.effort_claude
	);
	const configSummary = $derived(
		[
			form.account || 'default',
			effectiveAdapter,
			summaryModel || 'default',
			summaryEffort || 'default',
			form.permission_mode || 'default'
		].join(' · ')
	);

	// Per-mode accent: ask = green (safe), auto = blue (sandboxed),
	// yolo = red (no prompts, full access).
	const modeAccent: Record<string, string> = {
		ask: 'var(--c-green)',
		auto: 'var(--c-blue)',
		yolo: 'var(--c-red)',
		whip: 'var(--c-red)'
	};
</script>

<!-- Machine · working dir · label share one row (CCT-562). The label
     auto-fills from the (machine, cwd) memory (CCT-561, in SpawnModal). -->
<div class="top-row">
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

	<Field label="Label (optional)" for="sp-name">
		<Input id="sp-name" placeholder="session label" bind:value={form.name} />
	</Field>
</div>

<Field label="Prompt (optional)" for="sp-prompt">
	<Textarea
		id="sp-prompt"
		style="min-height:8rem;max-height:14rem;resize:vertical;overflow-y:auto"
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

<!-- Account · harness · model · effort · permission mode, collapsed into one
     line (CCT-562); the gear expands the full editors. Field ORDER inside the
     expansion is stable regardless of selection (CCT-404). -->
<div class="config">
	<div class="config-line">
		<Text size="sm" tone="faint" truncate title={configSummary}>{configSummary}</Text>
		<IconButton
			icon="settings"
			label="Session configuration"
			aria-expanded={configOpen}
			pressed={configOpen}
			onclick={() => (configOpen = !configOpen)}
		/>
	</div>

	{#if configOpen}
		<div class="config-fields">
			<Field label="Account" for="sp-account">
				<Select id="sp-account" bind:value={form.account}>
					<option value="">Default (no account)</option>
					{#each accounts as a (a.id)}
						<option value={a.name}>
							{a.name} ({a.providers.map((p) => p.provider).join(', ') || 'no provider'})
						</option>
					{/each}
				</Select>
				{#if selectedAccount}
					<Text tone="faint" size="xs">Runs through the passthrough gateway under this account.</Text>
				{:else}
					<Text tone="faint" size="xs">Default uses the worker's own auth.</Text>
				{/if}
			</Field>

			<!-- Harness: same two cards always; a card is disabled when the selected
			     account has no provider of that family (CCT-562), so the layout
			     never shifts. -->
			<Field label="Harness">
				<div class="adapters">
					{#each allAdapters as ad (ad)}
						<OptionButton
							row
							disabled={!!selectedAccount && !allowedAdapters.includes(ad)}
							selected={effectiveAdapter === ad}
							style="--opt-accent: {ad === 'codex' ? 'var(--c-blue)' : 'var(--c-amber)'}"
							onclick={() => {
								if (!selectedAccount || allowedAdapters.includes(ad)) form.adapter_id = ad;
							}}
						>
							<BrandLogo adapter={ad} size={18} />
							<Text>{adapterLabel(ad)}</Text>
						</OptionButton>
					{/each}
				</div>
				{#if selectedAccount && allowedAdapters.length < allAdapters.length}
					<Text tone="faint" size="xs">The selected account's providers limit the harness.</Text>
				{/if}
			</Field>

			<!-- Model: driven by the effective harness — the account's own models for
			     a compatible endpoint, else the harness's native families. -->
			<Field label="Model" for="sp-model">
				{#if usesAccountModels}
					<Select id="sp-model" bind:value={form.model_account}>
						{#if !accountModelOptions.length}
							<option value="">Default</option>
						{/if}
						{#each accountModelOptions as m (m.v)}<option value={m.v}>{m.label}</option>{/each}
					</Select>
				{:else if effectiveAdapter === 'codex'}
					<Select id="sp-model" bind:value={form.model_codex}>
						{#each codexModels as m (m.v)}<option value={m.v}>{m.label}</option>{/each}
					</Select>
				{:else}
					<Select id="sp-model" bind:value={form.model_claude}>
						{#each claudeModelOptions as m (m.v)}<option value={m.v}>{m.label}</option>{/each}
					</Select>
				{/if}
			</Field>

			<!-- Per-adapter effort: keyed off the effective harness so an
			     account-limited codex still shows codex efforts. -->
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
					<!-- "Default" (unset) leaves the mode to claude's own default — no mode
					     is forced into the spawn (CCT-542/CCT-558). -->
					<OptionButton
						selected={form.permission_mode === ''}
						onclick={() => (form.permission_mode = '')}
					>
						<strong>Default</strong>
						<Text tone="faint" size="xs">Claude default</Text>
					</OptionButton>
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
		</div>
	{/if}
</div>

<style>
	.top-row {
		display: grid;
		grid-template-columns: minmax(8rem, 1fr) minmax(0, 2fr) minmax(8rem, 1fr);
		gap: var(--sp-2);
		align-items: start;
	}
	@media (max-width: 640px) {
		.top-row {
			grid-template-columns: 1fr;
		}
	}
	.config {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.config-line {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
		min-width: 0;
		padding: var(--sp-1) var(--sp-2);
		border: 1px solid var(--border);
		border-radius: var(--r-md);
	}
	.config-fields {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
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
