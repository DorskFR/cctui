<script lang="ts">
	import { Button, Input, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import type { AccountModel } from '$lib/queries';
	import type { ModelOption } from '$lib/harnessModels';
	import ModelPicker from '$lib/components/molecules/ModelPicker.svelte';

	let {
		rows = $bindable([]),
		models = []
	}: {
		rows?: { alias: string; model: string }[];
		/** The provider's own catalog; empty ⇒ the target is free text. */
		models?: AccountModel[];
	} = $props();

	const options = $derived<ModelOption[]>(
		models.filter((mo) => mo.model).map((mo) => ({ v: mo.model, label: mo.label || mo.model }))
	);
</script>

<div class="page">
	<Text as="p" tone="muted" size="sm">{m.accounts_aliases_help()}</Text>

	{#each rows as row, i (i)}
		<div class="row">
			<Input
				bind:value={row.alias}
				mono
				size="sm"
				placeholder={m.accounts_placeholder_alias_name()}
				aria-label={m.a11y_alias_name()}
			/>
			<Text as="span" tone="faint" size="sm">→</Text>
			<div class="target">
				{#if options.length}
					<ModelPicker
						id="alias-model-{i}"
						compact
						bind:value={row.model}
						{options}
						aria-label={m.a11y_alias_model()}
					/>
				{:else}
					<Input
						bind:value={row.model}
						mono
						size="sm"
						placeholder={m.accounts_placeholder_alias_model()}
						aria-label={m.a11y_alias_model()}
					/>
				{/if}
			</div>
			<Button
				size="sm"
				variant="danger"
				aria-label={m.accounts_alias_remove_aria({ alias: row.alias })}
				onclick={() => (rows = rows.filter((_, j) => j !== i))}>✕</Button
			>
		</div>
	{/each}

	<div>
		<Button size="sm" onclick={() => (rows = [...rows, { alias: '', model: '' }])}>
			{m.accounts_add_alias()}
		</Button>
	</div>
</div>

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.row {
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto minmax(0, 1.4fr) auto;
		gap: var(--sp-2);
		align-items: center;
	}
	.target {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		min-width: 0;
	}
</style>
