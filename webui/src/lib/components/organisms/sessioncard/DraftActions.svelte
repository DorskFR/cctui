<script lang="ts">
	import { m } from '$lib/paraglide/messages';
	import { Button } from '@dorsk/tsumikit';
	import type { SessionActions, SessionView } from './view';

	let { view, actions }: { view: SessionView; actions: SessionActions } = $props();
</script>

<span
	class="draft-actions"
	role="presentation"
	onpointerdown={(e) => e.stopPropagation()}
	onclick={(e) => e.stopPropagation()}
>
	<Button size="sm" variant="primary" disabled={view.draftLaunching} onclick={() => actions.onLaunch?.(view.s)}>
		{#if view.draftLaunching}<span class="spin"></span>{/if}
		{m.sessions_launch()}
	</Button>
	<Button size="sm" onclick={() => actions.onEdit?.(view.s)}>{m.common_edit()}</Button>
	<Button size="sm" variant="danger" onclick={() => actions.onDiscard?.(view.s)}>{m.sessions_discard()}</Button>
</span>

<style>
	.draft-actions {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
		flex: none;
	}
</style>
