<script lang="ts">
	import type { TokenUsageWindows } from '@bindings/TokenUsageWindows';
	import { compact } from '$lib/format';
	import { m } from '$lib/paraglide/messages';
	import { Text } from '@dorsk/tsumikit';
	import { buildWindowRows, type WindowKey } from './WindowsTable.logic';

	let { windows }: { windows: TokenUsageWindows | undefined } = $props();

	const rows = $derived(buildWindowRows(windows));
	const labels: Record<WindowKey, () => string> = {
		hour: m.home_window_hour,
		today: m.home_window_today,
		day: m.home_window_day,
		week: m.home_window_week,
		month: m.home_window_month
	};
	const TH = 'font-size:10.5px;letter-spacing:.06em';
</script>

<div class="table">
	<div class="row">
		<div class="cell">
			<Text size="xs" tone="faint" uppercase style={TH}>{m.home_windows_col_window()}</Text>
		</div>
		<div class="cell bar">
			<Text size="xs" tone="faint" uppercase style={TH}>{m.home_windows_col_volume()}</Text>
		</div>
		<div class="cell end">
			<Text size="xs" tone="faint" uppercase nowrap style={TH}>↑ {m.home_usage_input()}</Text>
		</div>
		<div class="cell end">
			<Text size="xs" tone="faint" uppercase nowrap style={TH}>↓ {m.home_usage_output()}</Text>
		</div>
		<div class="cell end">
			<Text size="xs" tone="faint" uppercase nowrap style={TH}>⚡ {m.home_usage_cache()}</Text>
		</div>
	</div>
	{#each rows as r (r.key)}
		<div class="row">
			<div class="cell"><Text size="sm" truncate>{labels[r.key]()}</Text></div>
			<div class="cell bar">
				<div class="track"><div class="fill" style={`width:${r.share * 100}%`}></div></div>
			</div>
			<div class="cell end"><Text size="xs" numeric nowrap>{compact(r.input)}</Text></div>
			<div class="cell end"><Text size="xs" numeric nowrap>{compact(r.output)}</Text></div>
			<div class="cell end">
				<Text size="xs" tone="muted" numeric nowrap>{compact(r.cache_read)}</Text>
			</div>
		</div>
	{/each}
</div>

<style>
	.table {
		display: flex;
		flex-direction: column;
		container-type: inline-size;
	}
	.row {
		display: grid;
		grid-template-columns: 7.5rem minmax(0, 1fr) 5.625rem 5.625rem 6.25rem;
		gap: var(--sp-3);
		align-items: center;
		padding: var(--sp-2) var(--sp-4);
		border-bottom: 1px solid var(--border);
	}
	.row:last-child {
		border-bottom: 0;
	}
	.cell {
		min-width: 0;
	}
	.end {
		text-align: right;
	}
	/* The relative bar goes first when the column narrows: it only ranks the
	   rows, the three token figures carry the data. */
	@container (max-width: 40rem) {
		.row {
			grid-template-columns: minmax(0, 1fr) repeat(3, minmax(0, 3.5rem));
			gap: var(--sp-2);
			padding-inline: var(--sp-3);
		}
		.bar {
			display: none;
		}
	}
	.track {
		height: 0.375rem;
		border-radius: var(--r-pill);
		background: var(--bg);
		overflow: hidden;
	}
	.fill {
		height: 100%;
		border-radius: var(--r-pill);
		background: var(--accent-dim);
	}
</style>
