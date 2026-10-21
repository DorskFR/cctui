<script lang="ts">
	import type { SessionParentStub } from '@bindings/SessionParentStub';
	import { Button, Dot, Icon, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { parentStubTitle } from './sessions.logic';

	let {
		parent,
		count,
		open,
		ontoggle,
		onopenparent
	}: {
		parent: SessionParentStub;
		count: number;
		open: boolean;
		ontoggle: () => void;
		onopenparent: (id: string) => void;
	} = $props();

	const title = $derived(parentStubTitle(parent));
	const live = $derived(parent.status !== 'archived');
</script>

<div class="parent-group-header" data-parent-id={parent.id}>
	<Button
		variant="link"
		size="sm"
		title={m.sessions_parent_open({ name: title })}
		onclick={() => onopenparent(parent.id)}
	>
		{title}
	</Button>
	<span class="state">
		<Dot status={live ? 'active' : 'dead'} />
		<Text size="xs" tone="faint">{live ? m.sessions_parent_live() : m.sessions_status_archived()}</Text>
	</span>
	<Button variant="ghost" size="sm" aria-expanded={open} onclick={ontoggle}>
		{m.sessions_parent_children({ n: count })}
		<Icon name={open ? 'chevron-down' : 'chevron-right'} />
	</Button>
</div>

<style>
	.parent-group-header {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	.state {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
	}
</style>
