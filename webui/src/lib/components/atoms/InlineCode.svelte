<script module lang="ts">
	// Pass as the message param that stands in for the code span, so translators
	// keep control of where it lands in the sentence.
	export const SLOT = '\u0000';
</script>

<script lang="ts">
	import { Text } from '@dorsk/tsumikit';

	let { text, code }: { text: string; code: string } = $props();

	const parts = $derived(text.split(SLOT));
</script>

{#each parts as part, i (i)}{part}{#if i < parts.length - 1}<Text variant="code">{code}</Text>{/if}{/each}
