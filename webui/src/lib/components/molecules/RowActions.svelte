<script lang="ts">
	import type { Snippet } from 'svelte';

	let { children }: { children: Snippet } = $props();
</script>

<span class="acts">{@render children()}</span>

<style>
	.acts {
		display: flex;
		justify-content: flex-end;
		gap: 2px;
	}
</style>
