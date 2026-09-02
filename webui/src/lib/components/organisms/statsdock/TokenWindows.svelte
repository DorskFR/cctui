<script lang="ts">
	import { useTokenStats } from '$lib/queries';
	import TokenUsage from '$lib/components/molecules/TokenUsage.svelte';
	import { Text } from '@dorsk/tsumikit';
	import { asUsage } from '../../../../routes/home.logic';
	import { m } from '$lib/paraglide/messages';

	// The Overview's rolling token windows as one compact row each:
	// label on the left, the ↑in ↓out ⚡cache readout on the right.
	const tokens = useTokenStats();
	const rows = $derived([
		{ lbl: m.home_window_hour(), usage: asUsage(tokens.data?.hour) },
		{ lbl: m.home_window_today(), usage: asUsage(tokens.data?.today) },
		{ lbl: m.home_window_day(), usage: asUsage(tokens.data?.day) },
		{ lbl: m.home_window_week(), usage: asUsage(tokens.data?.week) },
		{ lbl: m.home_window_month(), usage: asUsage(tokens.data?.month) }
	]);
</script>

<dl class="windows">
	{#each rows as r (r.lbl)}
		<div class="win">
			<dt><Text size="xs" tone="muted">{r.lbl}</Text></dt>
			<dd><TokenUsage usage={r.usage} showSum={false} size="xs" wrap /></dd>
		</div>
	{/each}
</dl>

<style>
	.windows {
		display: flex;
		flex-direction: column;
		margin: 0;
	}
	.win {
		display: flex;
		justify-content: space-between;
		align-items: baseline;
		gap: var(--sp-2);
		padding: var(--sp-1) 0;
	}
	.win + .win {
		border-top: 1px solid var(--border);
	}
	.win dt {
		flex: none;
	}
	.win dd {
		margin: 0;
		min-width: 0;
		text-align: right;
	}
</style>
