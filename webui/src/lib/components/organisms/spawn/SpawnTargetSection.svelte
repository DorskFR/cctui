<script lang="ts">
	import { SegmentedControl } from '@dorsk/tsumikit';
	import { m } from '#lib/paraglide/messages.js';
	import type { SpawnForm } from './spawnForm.svelte';

	let { sf }: { sf: SpawnForm } = $props();
</script>

{#if sf.showTargetTabs}
	<SegmentedControl
		block
		label={m.spawn_run_on_label()}
		options={sf.targetOptions}
		value={sf.target}
		onchange={(v) => sf.setTarget(v)}
	/>
{/if}
