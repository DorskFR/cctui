<script lang="ts">
	let {
		count,
		tone = 'neutral',
		label
	}: { count: number; tone?: 'neutral' | 'unread'; label?: string } = $props();

	const shown = $derived(count > 99 ? '99+' : String(count));
</script>

{#if count > 0}
	<span
		class="count num"
		class:unread={tone === 'unread'}
		role={label ? 'img' : undefined}
		aria-label={label}>{shown}</span
	>
{/if}

<style>
	.count {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		flex: none;
		min-width: 1.5em;
		height: 1.5em;
		padding: 0 var(--sp-1);
		border-radius: var(--r-pill);
		background: var(--count-bg);
		color: var(--count-fg);
		font-size: var(--fs-xs);
		font-weight: var(--fw-semibold);
		line-height: 1;
	}
	.unread {
		background: var(--count-bg-unread);
		color: var(--count-fg-unread);
	}
</style>
