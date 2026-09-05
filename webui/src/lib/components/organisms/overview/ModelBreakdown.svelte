<script lang="ts">
	import type { ModelUsage } from '@bindings/ModelUsage';
	import { compact } from '$lib/format';
	import { m } from '$lib/paraglide/messages';
	import { Text } from '@dorsk/tsumikit';
	import { rankModels } from './usage-analytics';

	let { models }: { models: ModelUsage[] } = $props();

	const ranked = $derived(rankModels(models));
</script>

<div class="models">
	{#each ranked as r (r.model)}
		<div
			class="row"
			title={`${r.model}\n↑${r.input}  ↓${r.output}  ⚡${r.cache_read}\n${r.messages} ${m.home_usage_messages()}`}
		>
			<Text size="xs" truncate>{r.model}</Text>
			<div class="track">
				<div class="fill" style={`width:${Math.max(2, r.share * 100)}%`}></div>
			</div>
			<Text size="xs" tone="muted" numeric nowrap style="text-align:right">{compact(r.output)}</Text>
		</div>
	{/each}
</div>

<style>
	.models {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		container-type: inline-size;
	}
	.row {
		display: grid;
		grid-template-columns: minmax(0, 8.75rem) minmax(0, 1fr) 3.75rem;
		gap: var(--sp-2);
		align-items: center;
		min-width: 0;
	}
	/* The fixed name column becomes a share of the row on a narrow dock so the
	   bar keeps a usable track. */
	@container (max-width: 22rem) {
		.row {
			grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) 3rem;
		}
	}
	.track {
		height: 0.375rem;
		background: var(--bg);
		border-radius: var(--r-pill);
		overflow: hidden;
	}
	.fill {
		height: 100%;
		background: var(--info);
		border-radius: var(--r-pill);
	}
</style>
