<script lang="ts">
	import { AutoGrid, OptionButton, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import type { SpawnForm } from './spawnForm.svelte';

	let { sf }: { sf: SpawnForm } = $props();
</script>

{#if sf.canDispatch}
	<div class="switch-row">
		<AutoGrid min="8rem" maxCols={2} gap="var(--sp-2)" role="radiogroup" aria-label={m.spawn_run_on_label()}>
			{#each sf.targetOptions as o (o.value)}
				<OptionButton
					row
					selected={sf.target === o.value}
					role="radio"
					aria-checked={sf.target === o.value}
					onclick={() => sf.setTarget(o.value)}
				>
					<Text>{o.label}</Text>
				</OptionButton>
			{/each}
		</AutoGrid>
	</div>
{/if}

<style>
	.switch-row {
		display: block;
	}
</style>
