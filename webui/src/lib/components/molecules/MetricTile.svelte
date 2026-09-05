<script lang="ts">
	import { Card, Text } from '@dorsk/tsumikit';

	let {
		value,
		label,
		suffix,
		warn = false,
		compact = false
	}: {
		value: number | string;
		label: string;
		suffix?: string;
		warn?: boolean;
		compact?: boolean;
	} = $props();
</script>

<Card padding={compact ? 'sm' : 'md'}>
	<div class="tile" class:compact>
		<div class="num">
			<Text
				size={compact ? 'lg' : '2xl'}
				weight="semibold"
				numeric
				leading="none"
				tone={warn ? 'warn' : 'default'}
			>
				{value}
			</Text>
			{#if suffix}
				<Text size={compact ? 'xs' : 'sm'} tone="faint" numeric leading="none">{suffix}</Text>
			{/if}
		</div>
		<Text size="xs" tone="muted">{label}</Text>
	</div>
</Card>

<style>
	.tile {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		align-items: flex-start;
		min-width: 0;
	}
	.tile.compact {
		gap: var(--sp-1);
	}
	.num {
		display: flex;
		align-items: baseline;
		gap: 1px;
	}
</style>
