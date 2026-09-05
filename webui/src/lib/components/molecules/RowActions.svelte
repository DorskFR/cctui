<script lang="ts">
	import type { Snippet } from 'svelte';

	let { children }: { children: Snippet } = $props();
</script>

<span class="acts">{@render children()}</span>

<style>
	/* --act-o is set by the AccessTable row: 0 at rest, 1 on hover/focus and
	   always 1 on touch. */
	.acts {
		display: flex;
		justify-content: flex-end;
		gap: 2px;
		opacity: var(--act-o, 1);
		transition: opacity 0.12s var(--ease);
	}
</style>
