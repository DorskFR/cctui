<script lang="ts">
	import { Popover, Progress, Text } from '@dorsk/tsumikit';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { ws } from '#lib/ws.svelte.ts';
	import { usageKeys } from '#lib/queries/index.ts';
	import type { AccountUsageEntry } from '#lib/queries/index.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { patchUsageEntries } from './usage-battery.logic.ts';
	import { readingText, useUsageGauge, windowShort } from './usageGauge.svelte.ts';
	import UsageDetail from './UsageDetail.svelte';

	// Patched in place: invalidating would refetch and undo what the push saved.
	const qc = useQueryClient();
	$effect(() =>
		ws.onAccountUsage((ev) => {
			qc.setQueryData<AccountUsageEntry[]>(usageKeys.all(), (old) =>
				patchUsageEntries(old, ev.account_id, ev.usage)
			);
			qc.setQueryData(usageKeys.one(ev.account_id), ev.usage);
		})
	);

	const gauge = useUsageGauge();
	let open = $state(false);
</script>

{#if gauge.worst}
	{@const w = gauge.worst}
	<Popover
		bind:open
		label={m.usage_gauge_aria({ window: windowShort(w.window), pct: w.pct })}
		placement="bottom-end"
		bare
		hitArea="compact"
	>
		{#snippet trigger()}
			<span class="gauge" data-tone={w.tone}>
				<span class="meter" aria-hidden="true"><Progress value={w.pct} tone={w.tone} size="sm" /></span>
				<Text size="xs" numeric nowrap tone="inherit">{readingText(w)}</Text>
			</span>
		{/snippet}
		<div class="panel"><UsageDetail rows={gauge.rows} onnavigate={() => (open = false)} /></div>
	</Popover>
{/if}

<style>
	/* Lives in the px-pinned header: every length is px, never rem. */
	.gauge {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		height: 30px;
		padding: 0 8px;
		border-radius: 6px;
		color: var(--text-muted);
		background: var(--bg-elevated-2);
	}
	.panel {
		padding: var(--sp-2) var(--sp-3);
	}
	.gauge:hover {
		color: var(--text);
	}
	@media (pointer: coarse) {
		.gauge {
			min-height: 44px;
			min-width: 44px;
		}
	}
	.meter {
		display: block;
		width: 36px;
	}
	.gauge[data-tone='warn'] {
		color: var(--warn);
	}
	.gauge[data-tone='danger'] {
		color: var(--danger);
	}
</style>
