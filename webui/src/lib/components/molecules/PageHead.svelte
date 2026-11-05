<script lang="ts">
	// Every page opens the same way: the title (and an optional one-line
	// subtitle) on the left, its primary actions flush right, at one size and
	// one spacing (the Sessions bar is the reference).
	import type { Snippet } from 'svelte';
	import { Heading, Text } from '@dorsk/tsumikit';

	let { title, subtitle, children }: { title: string; subtitle?: string; children?: Snippet } = $props();
</script>

<div class="head">
	<div class="titles">
		<Heading level={1} size="xl">{title}</Heading>
		{#if subtitle}<Text tone="muted">{subtitle}</Text>{/if}
	</div>
	<span class="spacer"></span>
	{@render children?.()}
</div>

<style>
	.head {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: var(--sp-2);
		min-height: var(--control-height-default);
		margin-bottom: var(--sp-4);
	}
	.titles {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		min-width: 0;
	}
	.spacer {
		flex: 1;
	}
</style>
