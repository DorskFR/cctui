<script lang="ts">
	import { useAccountPoolsUsage } from '$lib/queries';
	import PoolUsageGauges from '$lib/components/organisms/accounts/PoolUsageGauges.svelte';
	import { Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	// Every pool the viewer can see, as its aggregate gauges: the level the
	// pool is at, its pace, and whether it holds until the members reset.
	const pools = useAccountPoolsUsage();
	const rows = $derived(pools.data ?? []);
</script>

{#if pools.isLoading}
	<Text tone="faint" size="sm">{m.common_loading()}</Text>
{:else if rows.length === 0}
	<Text tone="faint" size="sm">{m.stats_dock_no_pools()}</Text>
{:else}
	<div class="list">
		{#each rows as p (p.pool_id)}
			<section class="pool">
				<Text as="div" size="sm" weight="semibold" tone="accent">{p.name}</Text>
				<PoolUsageGauges usage={p} compact />
			</section>
		{/each}
	</div>
{/if}

<style>
	.list {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.pool {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
</style>
