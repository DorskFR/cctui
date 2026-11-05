<script lang="ts">
	import { SegmentedControl } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import type { SpawnForm } from './spawnForm.svelte';

	let { sf }: { sf: SpawnForm } = $props();
</script>

{#if sf.canDispatch}
	<SegmentedControl
		block
		control
		label={m.spawn_run_on_label()}
		options={sf.targetOptions}
		bind:value={() => sf.target, (v) => v && sf.setTarget(v)}
	/>
{/if}
