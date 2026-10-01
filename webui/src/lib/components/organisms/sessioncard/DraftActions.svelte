<script lang="ts">
	import { m } from '$lib/paraglide/messages';
	import { Button, Timestamp } from '@dorsk/tsumikit';
	import type { SessionActions, SessionView } from './view';

	let { view, actions }: { view: SessionView; actions: SessionActions } = $props();
</script>

<span
	class="draft-actions"
	role="presentation"
	onpointerdown={(e) => e.stopPropagation()}
	onclick={(e) => e.stopPropagation()}
>
	{#if view.scheduled}
		<span
			class="scheduled"
			class:failed={!!view.scheduled.error}
			title={view.scheduled.error ?? undefined}
			data-journey="draft-scheduled"
		>
			{#if view.scheduled.error}
				{m.sessions_draft_launch_failed({ error: view.scheduled.error })}
			{:else}
				{m.sessions_draft_launches_at()}
				<Timestamp value={view.scheduled.at} mode="datetime" tone="inherit" />
			{/if}
		</span>
	{/if}
	<Button
		size="sm"
		variant="primary"
		loading={view.draftLaunching}
		disabled={view.draftLaunching}
		onclick={() => actions.onLaunch?.(view.s)}
	>
		{view.scheduled ? m.sessions_launch_now() : m.sessions_launch()}
	</Button>
	{#if view.scheduled}
		<Button size="sm" onclick={() => actions.onCancelSchedule?.(view.s)}>
			{m.sessions_draft_cancel_schedule()}
		</Button>
	{/if}
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
	.scheduled {
		font-size: var(--fs-xs);
		color: var(--text-muted);
		white-space: nowrap;
	}
	.scheduled.failed {
		color: var(--danger);
	}
</style>
