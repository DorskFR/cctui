<script lang="ts">
	import { Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	let { pending }: { pending: { file: File }[] } = $props();
</script>

{#if pending.length}
	<div role="status" aria-live="polite">
		{#each pending as item}
			<Text size="xs">{item.file.name} : {m.attachments_compressing()}</Text>
		{/each}
	</div>
{/if}
