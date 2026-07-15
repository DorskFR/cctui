<script lang="ts">
	import type { ModelUsage } from '@bindings/ModelUsage';
	import { compact } from '$lib/format';
	import { m } from '$lib/paraglide/messages';
	import { Stack, Text } from '@dorsk/tsumikit';
	import { rankModels } from './usage-analytics';

	let { models }: { models: ModelUsage[] } = $props();

	const ranked = $derived(rankModels(models));
</script>

<Stack gap="var(--sp-2)">
	{#each ranked as r (r.model)}
		<div
			class="row"
			title={`${r.model}\n↑${r.input}  ↓${r.output}  ⚡${r.cache_read}\n${r.messages} ${m.home_usage_messages()}`}
		>
			<div class="meta">
				<Text size="sm" numeric truncate>{r.model}</Text>
				<Text size="xs" tone="faint" numeric>↓{compact(r.output)} · {compact(r.messages)} {m.home_usage_messages()}</Text>
			</div>
			<div class="track">
				<div class="fill" style={`width:${Math.max(2, r.share * 100)}%`}></div>
			</div>
		</div>
	{/each}
</Stack>

<style>
	.row {
		display: flex;
		flex-direction: column;
		gap: 0.2rem;
		min-width: 0;
	}
	.meta {
		display: flex;
		justify-content: space-between;
		gap: var(--sp-2);
		align-items: baseline;
		min-width: 0;
	}
	.track {
		height: 0.5rem;
		background: var(--bg-elevated-2, var(--bg-elevated));
		border-radius: var(--r-pill);
		overflow: hidden;
	}
	.fill {
		height: 100%;
		background: var(--accent);
		border-radius: var(--r-pill);
	}
</style>
