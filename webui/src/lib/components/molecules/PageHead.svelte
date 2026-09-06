<script lang="ts">
	// Every page opens the same way: the title on the left, its primary
	// actions flush right, at one size and one spacing (the Sessions bar is
	// the reference).
	import type { Snippet } from 'svelte';
	import { Heading } from '@dorsk/tsumikit';

	let { title, children }: { title: string; children?: Snippet } = $props();
</script>

<div class="head">
	<Heading level={1} size="xl" style="flex:none">{title}</Heading>
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
	.spacer {
		flex: 1;
	}
</style>
