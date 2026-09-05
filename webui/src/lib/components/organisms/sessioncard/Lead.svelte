<script lang="ts">
	import AccountBadge from '$lib/components/molecules/AccountBadge.svelte';
	import LabelBadge from '$lib/components/molecules/LabelBadge.svelte';
	import MachineBadge from '$lib/components/molecules/MachineBadge.svelte';
	import SessionDot from '$lib/components/molecules/SessionDot.svelte';
	import { m } from '$lib/paraglide/messages';
	import { settings } from '$lib/settings.svelte';
	import { Badge, Text, Timestamp } from '@dorsk/tsumikit';
	import { accountTrafficWarning } from '../../../../routes/sessions/sessions.logic';
	import Gutter from './Gutter.svelte';
	import type { SessionActions, SessionView } from './view';

	// gutter · dot · machine · account · title · labels · ⚙N cadence — the lead
	// group both the compact row and the detailed card header open with.
	let {
		view,
		actions,
		row = false
	}: {
		view: SessionView;
		actions: SessionActions;
		/** Compact row: capped title, no activity detail headline. */
		row?: boolean;
	} = $props();

	const s = $derived(view.s);
	const act = $derived(view.act);
</script>

<Gutter
	session={s}
	child={view.child}
	selectable={actions.selectable}
	selected={actions.selected}
	subagentToggles={actions.subagentToggles}
	onTogglePin={actions.onTogglePin}
/>
<SessionDot session={s} livenessClass={view.livenessClass} now={view.now} />
{#if view.child}
	<Badge tone="info" size="xs">{m.sessions_subagent_badge()}</Badge>
{:else if view.showMachine}
	<MachineBadge name={s.machine_name} id={s.machine_id} hue={s.machine_hue} mono />
{/if}
{#if !view.child}
	<AccountBadge name={s.account_name} warn={accountTrafficWarning(s)} showName={settings.accountNames} />
{/if}
<Text
	weight="semibold"
	size={row ? 'md' : 'lg'}
	truncate
	style={row ? 'flex:0 1 auto;min-width:0;max-width:min(28ch,40%)' : 'flex:0 1 auto;min-width:0'}
	>{view.title}</Text
>
{#if s.labels.length > 0 || actions.labelEditable}
	<LabelBadge
		labels={s.labels}
		editable={actions.labelEditable}
		allLabels={actions.allLabels}
		onCreate={actions.onCreateLabel}
		onAttach={(lid) => actions.onAttachLabel?.(s.id, lid)}
		onDetach={(lid) => actions.onDetachLabel?.(s.id, lid)}
		onUpdate={actions.onUpdateLabel}
		onDelete={actions.onDeleteLabel}
	/>
{/if}
{#if act.show && !view.stale}
	<span
		class="activity"
		class:asleep={act.asleep}
		title={act.detail ??
			(act.asleep ? m.sessions_activity_asleep_title() : m.sessions_activity_live_title())}
	>
		<span class="act-cadence"
			>⚙{act.count}{#if act.ageMs !== null && s.last_tool_at}&nbsp;·&nbsp;<Timestamp
					value={s.last_tool_at}
					mode="relative"
					size="xs"
					tone="inherit"
				/>{/if}</span
		>
		{#if act.detail && !row}<span class="act-detail">{act.detail}</span>{/if}
	</span>
{/if}

<style>
	.activity {
		display: inline-flex;
		align-items: baseline;
		gap: var(--sp-1);
		min-width: 0;
		flex: 0 1 auto;
		overflow: hidden;
		font-size: var(--fs-xs);
		color: var(--text-faint);
		white-space: nowrap;
	}
	@container sess-row (max-width: 40rem) {
		.activity {
			display: none;
		}
	}
	.act-cadence {
		flex: none;
		font-variant-numeric: tabular-nums;
	}
	.act-detail {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--text-muted);
		max-width: 22rem;
	}
	.activity.asleep,
	.activity.asleep .act-detail {
		color: var(--warn);
	}
</style>
