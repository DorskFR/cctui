<script lang="ts">
	import { Popover, Text } from '@dorsk/tsumikit';
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
				<span class="track"><span class="fill" style={`width: ${w.pct}%`}></span></span>
				<Text size="xs" numeric nowrap tone="inherit">{readingText(w)}</Text>
			</span>
		{/snippet}
		<UsageDetail rows={gauge.rows} onnavigate={() => (open = false)} />
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
	.gauge:hover {
		color: var(--text);
	}
	@media (pointer: coarse) {
		.gauge {
			min-height: 44px;
			min-width: 44px;
		}
	}
	.track {
		display: block;
		width: 24px;
		height: 4px;
		border-radius: 2px;
		background: var(--border);
		overflow: hidden;
	}
	.fill {
		display: block;
		height: 100%;
		border-radius: 2px;
		background: var(--text-faint);
	}
	.gauge[data-tone='warn'] {
		color: var(--warn);
	}
	.gauge[data-tone='warn'] .fill {
		background: var(--warn);
	}
	.gauge[data-tone='danger'] {
		color: var(--danger);
	}
	.gauge[data-tone='danger'] .fill {
		background: var(--danger);
	}
</style>
