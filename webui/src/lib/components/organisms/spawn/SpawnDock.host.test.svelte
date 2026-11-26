<script lang="ts">
	import { dockLayout, spawnDockMinified } from '#lib/spawnDock.svelte.ts';
	import SpawnDock from './SpawnDock.svelte';

	let { side = 'left' }: { side?: 'left' | 'right' } = $props();
</script>

<SpawnDock {side} width={dockLayout().spawnWidth ?? '30rem'} minified={spawnDockMinified()}>
	{#snippet body()}
		<input name="label" />
		<textarea name="draft"></textarea>
	{/snippet}
	{#snippet footer()}
		<button type="button">Launch</button>
	{/snippet}
</SpawnDock>
