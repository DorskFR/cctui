<script lang="ts">
	import { Card, Text } from '@dorsk/tsumikit';

	let {
		value,
		label,
		suffix,
		warn = false,
		compact = false,
		href,
		action
	}: {
		value: number | string;
		label: string;
		suffix?: string;
		warn?: boolean;
		compact?: boolean;
		href?: string;
		action?: { href: string; label: string };
	} = $props();
</script>

<div class="wrap" class:linked={!!href}>
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
			<span class="foot">
				{#if href}
					<a class="cover" {href}><Text size="xs" tone="muted">{label}</Text></a>
				{:else}
					<Text size="xs" tone="muted">{label}</Text>
				{/if}
				{#if action}<a class="action" href={action.href}>{action.label}</a>{/if}
			</span>
		</div>
	</Card>
</div>

<style>
	/* A grid so the kit Card stretches to the tallest tile in its row. */
	.wrap {
		position: relative;
		display: grid;
		height: 100%;
		border-radius: var(--r-md);
		transition: box-shadow var(--dur-fast, 120ms) var(--ease-out, ease);
	}
	.wrap.linked {
		cursor: pointer;
	}
	.wrap.linked:hover {
		box-shadow: 0 0 0 1px var(--border-strong);
	}
	.wrap:has(.cover:focus-visible) {
		outline: 2px solid var(--focus-ring, var(--accent));
		outline-offset: 2px;
	}
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
	.foot {
		display: flex;
		align-items: baseline;
		gap: var(--sp-2);
		min-width: 0;
	}
	.cover {
		color: inherit;
		text-decoration: none;
		outline: none;
	}
	/* The whole tile is the hit area of its link. */
	.cover::after {
		content: '';
		position: absolute;
		inset: 0;
	}
	.action {
		position: relative;
		z-index: 1;
		font-size: var(--fs-xs);
		color: var(--text-muted);
		text-decoration: underline;
		text-underline-offset: 2px;
	}
	.action:hover {
		color: var(--text);
	}
</style>
