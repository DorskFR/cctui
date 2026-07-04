<script lang="ts">
	// The "Dispatch (k8s)" branch of the spawn form, extracted from SpawnModal:
	// dispatcher selection + the fields forwarded to the dispatcher as `payload`
	// (name, identity, repo, ticket, prompt, prompt file, model, timeout, effort).
	// Dispatch runs a claude worker, so it uses the claude model/effort sets.
	import EffortSlider from './EffortSlider.svelte';
	import { Field, Input, Select, Text, Textarea } from '@dorsk/tsumikit';
	import { claudeModels, claudeEfforts, adapterForProvider, isCompatibleProvider, withAliasTargets } from './options';
	import { submitChordLabel, isSubmitChord } from '$lib/platform';
	import { primaryProvider, type OAuthAccount } from '$lib/queries';
	import type { Form } from './types';

	let {
		form = $bindable(),
		dispatcherIds,
		accounts,
		onsubmit
	}: {
		form: Form;
		dispatcherIds: string[];
		// The caller's accounts (CCT-399). Dispatch runs a claude worker, so only
		// Claude-family accounts can route through it.
		accounts: OAuthAccount[];
		// Submit the spawn form from the prompt textarea (Ctrl/⌘+Enter).
		onsubmit?: () => void;
	} = $props();

	// Dispatch runs a claude worker → only Claude-family accounts apply (CCT-399).
	// TODO(CCT-560/CCT-562): single-provider back-compat — reads providers[0].
	const providerOf = (a: OAuthAccount) => primaryProvider(a)?.provider ?? '';
	const dispatchAccounts = $derived(accounts.filter((a) => adapterForProvider(providerOf(a)) === 'claude-code'));
	const selectedAccount = $derived(
		form.account
			? (dispatchAccounts.find((a) => a.name === form.account && providerOf(a) === form.account_provider) ??
					dispatchAccounts.find((a) => a.name === form.account))
			: undefined
	);
	const selectedProvider = $derived(selectedAccount ? primaryProvider(selectedAccount) : undefined);
	const accountModelOptions = $derived((selectedProvider?.models ?? []).map((m) => ({ v: m.model, label: m.label })));
	const usesAccountModels = $derived(!!selectedProvider && isCompatibleProvider(selectedProvider.provider));
	// Native claude families annotated with the selected account's alias targets.
	const claudeModelOptions = $derived(withAliasTargets(claudeModels, selectedProvider?.model_aliases));

	$effect(() => {
		if (form.account && !dispatchAccounts.some((a) => a.name === form.account)) {
			form.account = '';
			form.account_provider = '';
		}
	});

	function onAccountChange(value: string) {
		form.account = value;
		const acct = value ? dispatchAccounts.find((a) => a.name === value) : undefined;
		form.account_provider = acct ? providerOf(acct) : '';
	}
</script>

{#if dispatcherIds.length >= 1}
	<Field label="Dispatcher" for="sp-dispatcher">
		<Select id="sp-dispatcher" bind:value={form.dispatcher}>
			{#each dispatcherIds as d (d)}<option value={d}>{d}</option>{/each}
		</Select>
	</Field>
{/if}

<Field label="Name (optional)" for="sp-name-d">
	<Input id="sp-name-d" placeholder="session label" bind:value={form.name} />
	<Text tone="faint" size="xs">Passed to the worker as <Text variant="code">--name</Text>.</Text>
</Field>

<Field label="Identity (optional)" for="sp-identity">
	<Input id="sp-identity" mono placeholder="alice" bind:value={form.identity} />
	<Text tone="faint" size="xs">Which account the worker acts as. Empty = worker default.</Text>
</Field>

<Field label="Repo" for="sp-repo">
	<Input id="sp-repo" mono placeholder="cctui" bind:value={form.repo} />
	<Text tone="faint" size="xs">Checked out under the worker's /workspace (optional).</Text>
</Field>

<Field label="Ticket (optional)" for="sp-ticket">
	<Input id="sp-ticket" mono placeholder="PROJ-1234" bind:value={form.ticket} />
	<Text tone="faint" size="xs">Issue id for the flow's context (e.g. an implement prompt).</Text>
</Field>

<Field label="Prompt" for="sp-prompt-d">
	<Textarea
		id="sp-prompt-d"
		style="min-height:8rem;max-height:60vh;resize:none;overflow-y:auto"
		placeholder="What should the worker do? (e.g. work on CCT-123 / a PR)"
		bind:value={form.prompt}
		autoresize
		onkeydown={(e: KeyboardEvent) => {
			if (onsubmit && isSubmitChord(e)) {
				e.preventDefault();
				onsubmit();
			}
		}}
	/>
	<Text size="xs" tone="faint" style="display:block;margin-top:var(--sp-1)">{submitChordLabel()} to dispatch</Text>
</Field>

<Field label="Prompt file (optional)" for="sp-prompt-file">
	<Input id="sp-prompt-file" mono placeholder="implement-from-ticket.md" bind:value={form.prompt_file} />
	<Text tone="faint" size="xs">A file under the worker's /prompts. Overrides the inline prompt.</Text>
</Field>

{#if dispatchAccounts.length}
	<Field label="Account (optional)" for="sp-account-d">
		<Select id="sp-account-d" value={form.account} onchange={(e) => onAccountChange((e.currentTarget as HTMLSelectElement).value)}>
			<option value="">Default (no account)</option>
			{#each dispatchAccounts as a (a.id)}<option value={a.name}>{a.name} ({providerOf(a)})</option>{/each}
		</Select>
		<Text tone="faint" size="xs">Routes the worker through the passthrough gateway under this account.</Text>
	</Field>
{/if}

<div class="row gap">
	<div class="grow">
		<Field label="Model" for="sp-model">
			{#if usesAccountModels}
				<Select id="sp-model" bind:value={form.model_account}>
					{#if !accountModelOptions.length}<option value="">Default</option>{/if}
					{#each accountModelOptions as m (m.v)}<option value={m.v}>{m.label}</option>{/each}
				</Select>
			{:else}
				<!-- Dispatch runs a claude worker → claude families. -->
				<Select id="sp-model" bind:value={form.model_claude}>
					{#each claudeModelOptions as m (m.v)}<option value={m.v}>{m.label}</option>{/each}
				</Select>
			{/if}
		</Field>
	</div>
	<div class="grow">
		<Field label="Timeout min" for="sp-timeout">
			<Input id="sp-timeout" mono inputmode="numeric" placeholder="default" bind:value={form.timeout} />
		</Field>
	</div>
</div>

<!-- Dispatch runs a claude worker → claude effort levels. -->
<EffortSlider
	id="sp-effort-d"
	levels={claudeEfforts}
	current={form.effort_claude}
	onset={(v) => (form.effort_claude = v)}
/>

<style>
	.row.gap {
		display: flex;
		gap: var(--sp-2);
	}
	.grow {
		flex: 1;
		min-width: 0;
	}
</style>
