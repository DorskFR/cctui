<script lang="ts">
	import { Icon, Popover } from '@dorsk/tsumikit';
	import { useSessionContext } from '$lib/queries';
	import SessionContextPanel from './SessionContextPanel.svelte';
	import { m } from '$lib/paraglide/messages';

	let {
		sessionId,
		canAttach,
		style = ''
	}: { sessionId: string; canAttach: boolean; style?: string } = $props();

	const ctx = useSessionContext(() => sessionId);
	const rules = $derived(ctx.data?.rules.length ?? 0);
	const memories = $derived(ctx.data?.memories.length ?? 0);
</script>

<Popover
	label={m.conversation_context_aria()}
	placement="bottom-end"
	variant="toggle"
	{style}
	data-journey="session-context"
>
	{#snippet trigger()}
		<Icon name="book" size={12} />
		<span class="wide">{m.conversation_context_chip({ rules, memories })}</span><span class="narrow"
			>{rules + memories}</span
		>
	{/snippet}
	<SessionContextPanel {sessionId} attached={ctx.data} {canAttach} />
</Popover>

<style>
	.narrow {
		display: none;
	}
	@container drawer-toolbar (max-width: 1000px) {
		.wide {
			display: none;
		}
		.narrow {
			display: inline;
		}
	}
</style>
