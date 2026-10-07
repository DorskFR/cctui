<script lang="ts">
	import type { EventRecord } from '@bindings/EventRecord';
	import { Badge, Dot, Text, Timestamp } from '@dorsk/tsumikit';
	import NavLink from '$lib/components/atoms/NavLink.svelte';
	import MachineBadge from '$lib/components/molecules/MachineBadge.svelte';
	import { formatRow, type ActorKind, type EventFamily } from '$lib/events';
	import { m } from '$lib/paraglide/messages';

	let {
		event,
		compact = false,
		showSubjects = true
	}: {
		event: EventRecord;
		/** One line, no subject chips: the per-session and per-machine lists
		 *  already know their subject. */
		compact?: boolean;
		showSubjects?: boolean;
	} = $props();

	const row = $derived(formatRow(event));

	const familyLabel: Record<EventFamily, () => string> = {
		session: m.events_family_session,
		machine: m.events_family_machine,
		system: m.events_family_system,
		other: m.events_family_other
	};
	const actorLabel: Record<ActorKind, () => string> = {
		user: m.events_actor_user,
		daemon: m.events_actor_daemon,
		reaper: m.events_actor_reaper,
		system: m.events_actor_system,
		agent: m.events_actor_agent
	};
</script>

<li class="row" class:compact data-kind={event.kind} data-severity={row.severity}>
	<Dot status={row.dot} label={row.severity} />
	<Timestamp value={row.occurredAt} mode="relative" short size="xs" tone="faint" />
	<Badge size="xs" tone={row.severity === 'info' ? 'neutral' : row.severity === 'warn' ? 'warn' : 'danger'} mono>
		{compact ? row.verb : `${familyLabel[row.family]()} · ${row.verb}`}
	</Badge>
	<Text size="sm" truncate title={row.summary}>{row.summary}</Text>
	{#if showSubjects && !compact}
		<span class="subjects">
			{#if row.sessionName || row.sessionHref}
				{#if row.sessionHref}
					<span class="link">
						<NavLink href={row.sessionHref}>
							<Badge size="xs" icon="list">{row.sessionName ?? m.events_session_fallback()}</Badge>
						</NavLink>
					</span>
				{:else}
					<Badge size="xs" icon="list" tone="muted" title={m.events_session_deleted()}>
						{row.sessionName ?? m.events_session_fallback()}
					</Badge>
				{/if}
			{/if}
			{#if row.machineId}
				<MachineBadge name={row.machineLabel} id={row.machineId} />
			{/if}
		</span>
	{/if}
	<Text size="xs" tone="faint" nowrap>{actorLabel[row.actor]()}</Text>
</li>

<style>
	.row {
		display: grid;
		grid-template-columns: auto auto auto minmax(0, 1fr) auto auto;
		align-items: center;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
		border-bottom: 1px solid var(--border-subtle, var(--border));
		min-width: 0;
	}
	.row.compact {
		grid-template-columns: auto auto auto minmax(0, 1fr) auto;
		padding: var(--sp-1) var(--sp-2);
	}
	.row:last-child {
		border-bottom: 0;
	}
	.subjects {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
		min-width: 0;
	}
	.link {
		display: inline-flex;
	}
</style>
