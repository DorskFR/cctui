<script lang="ts">
	import SessionMention from './SessionMention.svelte';
	import type { SessionListItem } from '@bindings/SessionListItem';
	let { sessions }: { sessions: SessionListItem[] } = $props();
	let value = $state('');
	let el = $state<HTMLTextAreaElement | null>(null);
</script>

<SessionMention bind:value {el} {sessions}>
	<textarea bind:value bind:this={el}></textarea>
</SessionMention>
