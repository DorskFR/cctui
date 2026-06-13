<script lang="ts">
	// The "Dispatch (k8s)" branch of the spawn form, extracted from SpawnModal:
	// dispatcher selection + the fields forwarded to the dispatcher as `payload`
	// (name, identity, repo, ticket, prompt, prompt file, model, timeout, effort).
	// Dispatch runs a claude worker, so it uses the claude model/effort sets.
	import EffortSlider from './EffortSlider.svelte';
	import Field from '$lib/components/molecules/Field.svelte';
	import Input from '$lib/components/atoms/Input.svelte';
	import Select from '$lib/components/atoms/Select.svelte';
	import Textarea from '$lib/components/atoms/Textarea.svelte';
	import { claudeModels, claudeEfforts } from './options';
	import type { Form } from './types';

	let {
		form = $bindable(),
		dispatcherIds
	}: {
		form: Form;
		dispatcherIds: string[];
	} = $props();
</script>

{#if dispatcherIds.length > 1}
	<Field label="Dispatcher" for="sp-dispatcher">
		<Select id="sp-dispatcher" bind:value={form.dispatcher}>
			{#each dispatcherIds as d (d)}<option value={d}>{d}</option>{/each}
		</Select>
	</Field>
{/if}

<Field label="Name (optional)" for="sp-name-d">
	<Input id="sp-name-d" placeholder="session label" bind:value={form.name} />
	<span class="faint sm">Passed to the worker as <code>--name</code>.</span>
</Field>

<Field label="Identity (optional)" for="sp-identity">
	<Input id="sp-identity" mono placeholder="alice" bind:value={form.identity} />
	<span class="faint sm">Which account the worker acts as. Empty = worker default.</span>
</Field>

<Field label="Repo" for="sp-repo">
	<Input id="sp-repo" mono placeholder="cctui" bind:value={form.repo} />
	<span class="faint sm">Checked out under the worker's /workspace (optional).</span>
</Field>

<Field label="Ticket (optional)" for="sp-ticket">
	<Input id="sp-ticket" mono placeholder="PROJ-1234" bind:value={form.ticket} />
	<span class="faint sm">Issue id for the flow's context (e.g. an implement prompt).</span>
</Field>

<Field label="Prompt" for="sp-prompt-d">
	<Textarea
		id="sp-prompt-d"
		style="min-height:8rem;max-height:60vh;resize:none;overflow-y:auto"
		placeholder="What should the worker do? (e.g. work on CCT-123 / a PR)"
		bind:value={form.prompt}
		autoresize
	/>
</Field>

<Field label="Prompt file (optional)" for="sp-prompt-file">
	<Input id="sp-prompt-file" mono placeholder="implement-from-ticket.md" bind:value={form.prompt_file} />
	<span class="faint sm">A file under the worker's /prompts. Overrides the inline prompt.</span>
</Field>

<div class="row gap">
	<div class="grow">
		<Field label="Model" for="sp-model">
			<!-- Dispatch runs a claude worker → claude families. -->
			<Select id="sp-model" bind:value={form.model_claude}>
				{#each claudeModels as m (m.v)}<option value={m.v}>{m.label}</option>{/each}
			</Select>
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
	.sm {
		font-size: var(--fs-xs);
	}
</style>
