<script lang="ts">
	import type { DockSide } from '$lib/dock';
	import { settings } from '$lib/settings.svelte';
	import DockGrip from '$lib/components/molecules/DockGrip.svelte';
	import AccountUsageList from './AccountUsageList.svelte';
	import TokenWindows from './TokenWindows.svelte';
	import OverviewTiles from './OverviewTiles.svelte';
	import UsageCharts from './UsageCharts.svelte';
	import { m } from '$lib/paraglide/messages';

	// The stats panel pinned to one edge of the Sessions screen (Settings ›
	// Stats panel): account usage gauges, rolling token windows, the Overview
	// counts and its charts, each in a foldable section. When `stacked`, the
	// spawn form owns the top half of the same column and this panel takes the
	// bottom half at the spawn panel's width. `width` is whatever the layout
	// reserved on this edge (resolveDocks), so the two never drift apart; the
	// grip on the inner edge writes a new width back to the settings, and a
	// stacked column resizes through the spawn panel's width.
	let {
		side,
		stacked = false,
		width
	}: { side: DockSide; stacked?: boolean; width: string } = $props();

	function setWidth(px: number | undefined) {
		if (stacked) settings.setSpawnDock({ width: px });
		else settings.setStatsDock({ width: px });
	}

	const sections = [
		{ key: 'accounts', title: () => m.stats_dock_accounts(), open: true },
		{ key: 'tokens', title: () => m.home_token_usage(), open: true },
		{ key: 'overview', title: () => m.home_overview_title(), open: true },
		{ key: 'usage', title: () => m.home_usage_title(), open: false }
	];
</script>

<aside
	class="dock"
	class:dock-left={side === 'left'}
	class:stacked
	style:--stats-dock-w={width}
	aria-label={m.stats_dock_title()}
>
	<DockGrip {side} onwidth={setWidth} onreset={() => setWidth(undefined)} />
	<div class="dock-head">{m.stats_dock_title()}</div>
	<div class="dock-body">
		{#each sections as s (s.key)}
			<details class="section" open={s.open}>
				<summary>{s.title()}</summary>
				<div class="section-body">
					{#if s.key === 'accounts'}
						<AccountUsageList />
					{:else if s.key === 'tokens'}
						<TokenWindows />
					{:else if s.key === 'overview'}
						<OverviewTiles />
					{:else}
						<UsageCharts />
					{/if}
				</div>
			</details>
		{/each}
	</div>
</aside>

<style>
	.dock {
		position: fixed;
		top: calc(var(--header-h) + var(--safe-top));
		bottom: calc(var(--nav-h) + var(--safe-bottom));
		right: 0;
		width: var(--stats-dock-w);
		display: flex;
		flex-direction: column;
		background: var(--bg-elevated);
		border-left: 1px solid var(--border);
		z-index: 4;
	}
	.dock.dock-left {
		right: auto;
		left: 0;
		border-left: 0;
		border-right: 1px solid var(--border);
	}
	/* Sharing the column with the spawn form: bottom half only. */
	.dock.stacked {
		top: 50%;
		border-top: 1px solid var(--border);
	}
	.dock-head {
		flex: none;
		padding: var(--sp-3) var(--sp-3) var(--sp-2);
		font-weight: var(--fw-semibold);
		border-bottom: 1px solid var(--border);
	}
	.dock-body {
		flex: 1;
		min-height: 0;
		overflow-y: auto;
		padding: var(--sp-2) var(--sp-3) var(--sp-3);
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	.section {
		border-bottom: 1px solid var(--border);
		padding-bottom: var(--sp-2);
	}
	.section:last-child {
		border-bottom: 0;
	}
	.section summary {
		cursor: pointer;
		padding: var(--sp-2) 0;
		font-size: var(--fs-xs);
		font-weight: var(--fw-semibold);
		text-transform: uppercase;
		letter-spacing: 0.04em;
		color: var(--text-muted);
		user-select: none;
	}
	.section-body {
		padding-top: var(--sp-1);
	}
</style>
