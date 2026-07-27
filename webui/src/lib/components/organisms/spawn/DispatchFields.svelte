<script lang="ts">
	// The "Dispatch (k8s)" branch of the spawn form, extracted from SpawnModal:
	// dispatcher + adapter selection and the fields forwarded to the dispatcher as
	// `payload` (name, identity, repo, ticket, prompt, prompt file, model, timeout,
	// effort). The adapter picker chooses the claude or codex worker; the
	// model/effort sets follow it.
	import EffortSlider from './EffortSlider.svelte';
	import { Field, Input, Select, Text, Textarea } from '@dorsk/tsumikit';
	import {
		claudeModels,
		claudeEfforts,
		codexModels,
		codexEfforts,
		accountAdapters,
		providerForAdapter,
		isCompatibleProvider,
		withAliasTargets,
		allAdapters,
		adapterLabel,
		type Adapter
	} from './options';
	import { submitChordLabel, isSubmitChord } from '$lib/platform';
	import type { OAuthAccount } from '$lib/queries';
	import type { Form } from './types';
	import { m } from '$lib/paraglide/messages';

	let {
		form = $bindable(),
		dispatcherIds,
		accounts,
		onsubmit
	}: {
		form: Form;
		dispatcherIds: string[];
		// The caller's accounts. Filtered per selected adapter below.
		accounts: OAuthAccount[];
		// Submit the spawn form from the prompt textarea (Ctrl/⌘+Enter).
		onsubmit?: () => void;
	} = $props();

	const adapter = $derived(form.dispatch_adapter || 'claude-code');
	const isCodex = $derived(adapter === 'codex');

	// Only accounts whose provider family backs the selected harness apply
	// (provider-family union): a claude worker needs an anthropic-family
	// provider, a codex worker an openai-family one.
	const dispatchAccounts = $derived(accounts.filter((a) => accountAdapters(a).includes(adapter as Adapter)));
	const selectedAccount = $derived(
		form.account ? dispatchAccounts.find((a) => a.name === form.account) : undefined
	);
	const selectedProvider = $derived(providerForAdapter(selectedAccount, adapter));
	const accountModelOptions = $derived((selectedProvider?.models ?? []).map((m) => ({ v: m.model, label: m.label })));
	const usesAccountModels = $derived(!!selectedProvider && isCompatibleProvider(selectedProvider.provider));
	// Native families for the selected harness. Codex dispatch uses the static
	// offline catalog (an ephemeral worker has no machine-scoped catalog); claude
	// families are annotated with the account's alias targets.
	const nativeModelOptions = $derived(
		isCodex ? codexModels : withAliasTargets(claudeModels, selectedProvider?.model_aliases)
	);
	const nativeEfforts = $derived(isCodex ? codexEfforts : claudeEfforts);

	$effect(() => {
		if (form.account && !dispatchAccounts.some((a) => a.name === form.account)) {
			form.account = '';
		}
	});
	// Keep account_provider tracking the credential actually in use.
	$effect(() => {
		form.account_provider = selectedProvider?.provider ?? '';
	});
</script>

{#if dispatcherIds.length >= 1}
	<Field label={m.dispatch_dispatcher_label()} for="sp-dispatcher">
		<Select id="sp-dispatcher" bind:value={form.dispatcher}>
			{#each dispatcherIds as d (d)}<option value={d}>{d}</option>{/each}
		</Select>
	</Field>
{/if}

<Field label={m.spawn_field_harness()} for="sp-adapter-d">
	<Select id="sp-adapter-d" bind:value={form.dispatch_adapter}>
		{#each allAdapters as a (a)}<option value={a}>{adapterLabel(a)}</option>{/each}
	</Select>
	<Text tone="faint" size="xs">
		{isCodex ? m.dispatch_harness_codex_hint() : m.dispatch_harness_claude_hint()}
	</Text>
</Field>

<Field label={m.dispatch_name_label()} for="sp-name-d">
	<Input id="sp-name-d" placeholder={m.spawn_session_label_placeholder()} bind:value={form.name} />
	<Text tone="faint" size="xs">{m.dispatch_name_hint_pre()}<Text variant="code">--name</Text>{m.dispatch_name_hint_post()}</Text>
</Field>

<Field label={m.dispatch_identity_label()} for="sp-identity">
	<Input id="sp-identity" mono placeholder="alice" bind:value={form.identity} />
	<Text tone="faint" size="xs">{m.dispatch_identity_hint()}</Text>
</Field>

<Field label={m.dispatch_repo_label()} for="sp-repo">
	<Input id="sp-repo" mono placeholder="cctui" bind:value={form.repo} />
	<Text tone="faint" size="xs">{m.dispatch_repo_hint()}</Text>
</Field>

<Field label={m.dispatch_ticket_label()} for="sp-ticket">
	<Input id="sp-ticket" mono placeholder="PROJ-1234" bind:value={form.ticket} />
	<Text tone="faint" size="xs">{m.dispatch_ticket_hint()}</Text>
</Field>

<Field label={m.dispatch_prompt_label()} for="sp-prompt-d">
	<Textarea
		id="sp-prompt-d"
		style="min-height:8rem;max-height:60vh;resize:none;overflow-y:auto"
		placeholder={m.dispatch_prompt_placeholder()}
		bind:value={form.prompt}
		autoresize
		onkeydown={(e: KeyboardEvent) => {
			if (onsubmit && isSubmitChord(e)) {
				e.preventDefault();
				onsubmit();
			}
		}}
	/>
	<Text size="xs" tone="faint" style="display:block;margin-top:var(--sp-1)">{m.dispatch_prompt_submit_hint({ chord: submitChordLabel() })}</Text>
</Field>

<Field label={m.dispatch_prompt_file_label()} for="sp-prompt-file">
	<Input id="sp-prompt-file" mono placeholder="implement-from-ticket.md" bind:value={form.prompt_file} />
	<Text tone="faint" size="xs">{m.dispatch_prompt_file_hint()}</Text>
</Field>

{#if dispatchAccounts.length}
	<Field label={m.dispatch_account_label()} for="sp-account-d">
		<Select id="sp-account-d" bind:value={form.account}>
			<option value="">{m.dispatch_account_default()}</option>
			{#each dispatchAccounts as a (a.id)}
				<option value={a.name}>{a.name} ({providerForAdapter(a, adapter)?.provider})</option>
			{/each}
		</Select>
		<Text tone="faint" size="xs">{m.dispatch_account_hint()}</Text>
	</Field>
{/if}

<div class="row gap">
	<div class="grow">
		<Field label={m.spawn_field_model()} for="sp-model">
			{#if usesAccountModels}
				<Select id="sp-model" bind:value={form.model_account}>
					{#if !accountModelOptions.length}<option value="">{m.spawn_model_default()}</option>{/if}
					{#each accountModelOptions as m (m.v)}<option value={m.v}>{m.label}</option>{/each}
				</Select>
			{:else if isCodex}
				<Select id="sp-model" bind:value={form.model_codex}>
					{#each nativeModelOptions as m (m.v)}<option value={m.v}>{m.label}</option>{/each}
				</Select>
			{:else}
				<Select id="sp-model" bind:value={form.model_claude}>
					{#each nativeModelOptions as m (m.v)}<option value={m.v}>{m.label}</option>{/each}
				</Select>
			{/if}
		</Field>
	</div>
	<div class="grow">
		<Field label={m.dispatch_timeout_label()} for="sp-timeout">
			<Input id="sp-timeout" mono inputmode="numeric" placeholder={m.dispatch_timeout_placeholder()} bind:value={form.timeout} />
		</Field>
	</div>
</div>

{#if isCodex}
	<EffortSlider
		id="sp-effort-d"
		levels={nativeEfforts}
		current={form.effort_codex}
		onset={(v) => (form.effort_codex = v)}
	/>
{:else}
	<EffortSlider
		id="sp-effort-d"
		levels={nativeEfforts}
		current={form.effort_claude}
		onset={(v) => (form.effort_claude = v)}
	/>
{/if}

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
