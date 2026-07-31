<script lang="ts">
	import { net, formatBytes, wireTotals, type WireTotals } from '$lib/netstats.svelte';
	import { clickOutside } from '$lib/clickOutside';
	import { Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let open = $state(false);
	let wire = $state<WireTotals | null>(null);
	$effect(() => {
		if (open) wire = wireTotals();
	});
	const topRoutes = $derived(open ? net.routes().slice(0, 8) : []);
</script>

<div class="netstats" use:clickOutside={() => (open = false)}>
	<button
		type="button"
		class="net-chip"
		title={m.net_stats_title()}
		aria-expanded={open}
		onclick={() => (open = !open)}
	>
		<Text size="xs" tone="faint" variant="code">↓ {formatBytes(net.total)}</Text>
	</button>
	{#if open}
		<div class="net-panel">
			<div class="net-head"><Text size="xs" weight="bold">{m.net_stats_title()}</Text></div>
			<div class="net-row">
				<span>{m.net_stats_api()}</span>
				<span>{formatBytes(net.apiBytes)} · {m.net_stats_requests({ count: net.apiCount })}</span>
			</div>
			<div class="net-row">
				<span>{m.net_stats_ws()}</span>
				<span>{formatBytes(net.wsBytes)} · {m.net_stats_requests({ count: net.wsCount })}</span>
			</div>
			{#if wire && wire.count > 0}
				<div class="net-row">
					<span>{m.net_stats_wire()}</span>
					<span>{formatBytes(wire.transfer)} → {formatBytes(wire.decoded)}</span>
				</div>
			{/if}
			{#if topRoutes.length > 0}
				<div class="net-sep"></div>
				{#each topRoutes as r (r.route)}
					<div class="net-row route">
						<span class="net-route">{r.route}</span>
						<span>{formatBytes(r.bytes)} · {m.net_stats_requests({ count: r.count })}</span>
					</div>
				{/each}
			{/if}
		</div>
	{/if}
</div>

<style>
	.netstats {
		position: relative;
	}
	.net-chip {
		border: none;
		background: none;
		padding: 0 var(--sp-1);
		cursor: pointer;
		white-space: nowrap;
	}
	.net-panel {
		position: absolute;
		top: calc(100% + 6px);
		right: 0;
		z-index: var(--z-header);
		min-width: 280px;
		max-width: 90vw;
		padding: var(--sp-2) var(--sp-3);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-2, 6px);
		background: var(--bg-elevated-2);
		box-shadow: var(--shadow-md);
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	.net-head {
		margin-bottom: var(--sp-1);
	}
	.net-row {
		display: flex;
		justify-content: space-between;
		gap: var(--sp-3);
		font-size: var(--fs-xs);
		color: var(--text-muted);
		font-family: var(--font-mono, monospace);
	}
	.net-row.route {
		color: var(--text);
	}
	.net-route {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.net-sep {
		height: 1px;
		background: var(--border);
		margin: var(--sp-1) 0;
	}
</style>
