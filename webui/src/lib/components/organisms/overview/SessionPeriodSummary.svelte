<script lang="ts">
	import type { SessionStats } from '@bindings/SessionStats';
	import { Card, Stack, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { getLocale } from '$lib/paraglide/runtime';

	let { stats }: { stats?: SessionStats } = $props();
	const periods = $derived([
		{ key: 'today', label: m.home_sessions_today(), value: stats?.today },
		{ key: 'yesterday', label: m.home_sessions_yesterday(), value: stats?.yesterday },
		{ key: 'week', label: m.home_sessions_week(), value: stats?.week },
		{ key: 'month', label: m.home_sessions_month(), value: stats?.month }
	]);
</script>

<Card data-journey="session-periods">
	<Stack gap="var(--sp-4)">
		<Text size="sm" weight="semibold">{m.home_sessions_periods_title()}</Text>
		<div class="periods">
			{#each periods as period (period.key)}
				<div class="period" data-journey-key={period.key}>
					<Text size="2xl" weight="semibold" numeric leading="none" tone={period.key === 'today' ? 'accent' : 'default'}>
						{period.value == null ? '…' : period.value.toLocaleString(getLocale())}
					</Text>
					<Text size="xs" tone="muted">{period.label}</Text>
				</div>
			{/each}
		</div>
		<Text size="xs" tone="faint">{m.home_sessions_periods_hint()}</Text>
	</Stack>
</Card>

<style>
	.periods {
		display: grid;
		grid-template-columns: repeat(4, minmax(0, 1fr));
		gap: var(--sp-4);
	}
	.period {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding-inline-start: var(--sp-3);
		border-inline-start: thin solid var(--border);
	}
	@media (max-width: 40rem) {
		.periods { grid-template-columns: repeat(2, minmax(0, 1fr)); }
	}
</style>
