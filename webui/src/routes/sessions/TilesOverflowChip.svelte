<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { Button, Popover, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		items,
		onOpen
	}: {
		items: SessionListItem[];
		/** Open a session the grid had no room for, in the normal drawer. */
		onOpen?: (s: SessionListItem) => void;
	} = $props();
</script>

<Popover
	label={m.tiles_more({ count: items.length })}
	variant="default"
	tone="accent"
	size="sm"
	pill
>
	{#snippet trigger()}{m.tiles_more({ count: items.length })}{/snippet}
	<div class="more-list">
		<Text size="xs" tone="muted">{m.tiles_more_help()}</Text>
		{#each items as s (s.id)}
			<Button
				size="sm"
				variant="ghost"
				block
				style="justify-content:flex-start"
				onclick={() => onOpen?.(s)}
			>
				{s.name || s.working_dir}
			</Button>
		{/each}
	</div>
</Popover>

<style>
	.more-list {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		max-height: 50vh;
		overflow-y: auto;
		overflow-x: hidden;
		min-width: 16rem;
	}
</style>
