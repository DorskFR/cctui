<script lang="ts">
	import { Button, Input, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import type { AccountModel } from '$lib/queries';
	import FireworksProviderEditor from '$lib/components/organisms/FireworksProviderEditor.svelte';

	let {
		fireworks = false,
		models = $bindable([]),
		settings = $bindable({})
	}: {
		fireworks?: boolean;
		models?: AccountModel[];
		settings?: Record<string, unknown>;
	} = $props();

	function set(i: number, key: 'model' | 'label', value: string) {
		const next = [...models];
		next[i] = { ...next[i], [key]: value };
		models = next;
	}
</script>

{#if fireworks}
	<FireworksProviderEditor section="models" bind:settings bind:models />
{:else}
	<div class="page">
		<Text as="p" tone="muted" size="sm">{m.accounts_models_help()}</Text>
		{#each models as row, i (i)}
			<div class="row">
				<Input
					value={row.model}
					mono
					size="sm"
					placeholder={m.accounts_placeholder_model_code()}
					aria-label={m.a11y_model_code()}
					oninput={(e: Event) => set(i, 'model', (e.currentTarget as HTMLInputElement).value)}
				/>
				<Input
					value={row.label}
					size="sm"
					placeholder={m.accounts_placeholder_model_label()}
					aria-label={m.a11y_model_label()}
					oninput={(e: Event) => set(i, 'label', (e.currentTarget as HTMLInputElement).value)}
				/>
				<Button
					size="sm"
					variant="danger"
					aria-label={m.accounts_model_remove_aria({ model: row.model })}
					onclick={() => (models = models.filter((_, j) => j !== i))}>✕</Button
				>
			</div>
		{/each}
		<div>
			<Button size="sm" onclick={() => (models = [...models, { model: '', label: '' }])}>
				{m.accounts_add_model()}
			</Button>
		</div>
	</div>
{/if}

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.row {
		display: grid;
		grid-template-columns: minmax(0, 1.6fr) minmax(0, 1fr) auto;
		gap: var(--sp-2);
		align-items: center;
	}
</style>
