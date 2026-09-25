<script lang="ts">
	let { texts }: { texts: string[] } = $props();
</script>

<div class="marker-body">
	{#each texts as mt, i (i)}
		<span class="marker-item">{mt}</span>
	{/each}
</div>

<style>
	/* Markers are bookkeeping, not messages: one quiet line, no bubble. */
	.marker-body {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-2);
		color: var(--text-faint);
		font-size: var(--fs-xs);
		line-height: 1.4;
	}
	.marker-item::before {
		content: '· ';
	}
</style>
