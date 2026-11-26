<script lang="ts">
	import { Link, Progress, Text } from '@dorsk/tsumikit';
	import AdapterIcon from '#lib/components/atoms/AdapterIcon.svelte';
	import { now as clockNow } from '#lib/clock.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { countdown } from './usage-battery.logic.ts';
	import type { GaugeReading } from './usage-gauge.logic.ts';
	import { windowShort, type GaugeRow } from './usageGauge.svelte.ts';

	let { rows, onnavigate }: { rows: GaugeRow[]; onnavigate?: () => void } = $props();

	const now = $derived(clockNow(30_000));
	const resetTitle = (r: GaugeReading) => {
		const ms = r.resetsAt ? Date.parse(r.resetsAt) - now : Number.NaN;
		return Number.isFinite(ms) ? m.usage_battery_resets_in({ time: countdown(ms) }) : undefined;
	};
</script>

<div class="detail">
	<ul class="rows">
		{#each rows as row (row.entry.providerId)}
			<li class="row">
				<span class="who">
					<AdapterIcon provider={row.entry.provider} size={14} />
					<Text size="sm" truncate>{row.entry.accountName}</Text>
				</span>
				<span class="vals">
					{#each row.readings as r (r.window)}
						<span class="reading" title={resetTitle(r)}>
							<Text size="xs" tone="muted">{windowShort(r.window)}</Text>
							<span class="meter" aria-hidden="true"><Progress value={r.pct} tone={r.tone} size="sm" /></span>
							<span class="pct"
								><Text size="sm" numeric tone={r.tone === 'ok' ? 'default' : r.tone}>{r.pct}%</Text></span
							>
						</span>
					{/each}
				</span>
			</li>
		{/each}
	</ul>
	<Link href="/accounts" onclick={onnavigate}>{m.usage_gauge_open_accounts()}</Link>
</div>

<style>
	.detail {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 16rem;
		max-width: 100%;
	}
	.rows {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-3);
	}
	.who {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	.vals {
		display: inline-flex;
		gap: var(--sp-4);
		flex: none;
	}
	.reading {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.meter {
		display: block;
		width: 32px;
	}
	.pct {
		min-width: 3.5ch;
		text-align: right;
	}
</style>
