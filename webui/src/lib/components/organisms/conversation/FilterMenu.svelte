<script lang="ts">
	import { Button, Checkbox, Text } from '@dorsk/tsumikit';
	import { MSG_GROUPS } from './filters';
	import { msgCategoryLabel, msgGroupLabel, type MsgCategory, type MsgFilter } from './types';
	import { m } from '$lib/paraglide/messages';

	let {
		filter,
		ontoggle,
		onall
	}: {
		filter: MsgFilter;
		ontoggle: (c: MsgCategory) => void;
		onall: (on: boolean) => void;
	} = $props();
</script>

<div class="menu">
	<div class="bulk">
		<Button variant="ghost" onclick={() => onall(true)}>{m.conversation_filter_all()}</Button>
		<Button variant="ghost" onclick={() => onall(false)}>{m.conversation_filter_none()}</Button>
	</div>
	{#each MSG_GROUPS as g (g.id)}
		<div class="group" role="group" aria-label={msgGroupLabel(g.id)}>
			<Text tone="faint" size="xs" weight="medium">{msgGroupLabel(g.id)}</Text>
			{#each g.categories as c (c)}
				<Checkbox
					label={msgCategoryLabel(c)}
					checked={filter[c]}
					onchange={() => ontoggle(c)}
				/>
			{/each}
		</div>
	{/each}
</div>

<style>
	.menu {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		min-width: 13rem;
		max-height: min(60vh, 28rem);
		overflow-y: auto;
		padding: var(--sp-1);
	}
	.bulk {
		display: flex;
		gap: var(--sp-1);
		padding-bottom: var(--sp-1);
		border-bottom: 1px solid var(--border);
	}
	.group {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
</style>
