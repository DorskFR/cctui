<script lang="ts">
	import LabelBadge from '$lib/components/molecules/LabelBadge.svelte';
	import SessionDot from '$lib/components/molecules/SessionDot.svelte';
	import SessionGlyphs from '$lib/components/molecules/SessionGlyphs.svelte';
	import StatText from '$lib/components/atoms/StatText.svelte';
	import { m } from '$lib/paraglide/messages';
	import { settings } from '$lib/settings.svelte';
	import { Text, Timestamp } from '@dorsk/tsumikit';
	import { accountTrafficWarning } from '../../../../routes/sessions/sessions.logic';
	import Gutter from './Gutter.svelte';
	import { needsYouOf, type SessionActions, type SessionView } from './view';

	// Card: gutter · star/dot/machine/account glyphs · title · labels · activity.
	// Row: gutter · dot · title; the row owns the rest of its line.
	let {
		view,
		actions,
		row = false
	}: {
		view: SessionView;
		actions: SessionActions;
		row?: boolean;
	} = $props();

	const s = $derived(view.s);
	const act = $derived(view.act);
	// The in-progress task's activeForm is the more specific version of the
	// daemon's spinner text, so it wins the single bounded headline slot.
	const headline = $derived(act.todoActive ?? act.detail);
	const needsYou = $derived(needsYouOf(s));
</script>

<Gutter
	child={view.child}
	selectable={actions.selectable}
	selected={actions.selected}
	subagentToggles={actions.subagentToggles}
/>
{#if view.child || row}
	<SessionDot session={s} livenessClass={view.livenessClass} now={view.now} />
{:else}
	<SessionGlyphs
		session={s}
		livenessClass={view.livenessClass}
		now={view.now}
		stack="never"
		showMachine={view.showMachine}
		accountWarn={accountTrafficWarning(s)}
		showAccountName={settings.accountNames}
		onTogglePin={actions.selectable ? undefined : actions.onTogglePin}
	/>
{/if}
<span class="title" class:row>
	<Text
		data-journey="title"
		weight="semibold"
		size={row ? 'md' : 'lg'}
		truncate
		style="min-width:0;max-width:100%">{view.title}</Text
	>
</span>
{#if !row && (s.labels.length > 0 || actions.labelEditable)}
	<span class="labels" class:empty={s.labels.length === 0}><LabelBadge
		labels={s.labels}
		editable={actions.labelEditable}
		allLabels={actions.allLabels}
		onCreate={actions.onCreateLabel}
		onAttach={(lid) => actions.onAttachLabel?.(s.id, lid)}
		onDetach={(lid) => actions.onDetachLabel?.(s.id, lid)}
		onUpdate={actions.onUpdateLabel}
		onDelete={actions.onDeleteLabel}
	/></span>
{/if}
{#if !row && act.show && !view.stale}
	<span
		class="activity"
		class:asleep={act.asleep}
		title={act.todoActive ??
			act.detail ??
			(act.asleep ? m.sessions_activity_asleep_title() : m.sessions_activity_live_title())}
	>
		{#if act.count > 0 || act.ageMs !== null}
			<span class="act-cadence"
				><StatText value={act.count} kind="tools" />{#if act.ageMs !== null && s.last_tool_at}&nbsp;·&nbsp;<Timestamp
						value={s.last_tool_at}
						mode="relative"
						size="xs"
						tone="inherit"
					/>{/if}</span
			>
		{/if}
		{#if act.todoTotal > 0}
			<span class="act-todos" class:running={act.todoActive !== null}
				>{act.todoDone}/{act.todoTotal}</span
			>
		{/if}
		{#if needsYou}
			<span
				class="act-needs"
				class:blocking={needsYou.blocking}
				title={needsYou.child > 0
					? m.sessions_needs_you_child_title({ count: needsYou.child })
					: m.sessions_needs_you_title()}
				>{m.sessions_needs_you({ count: needsYou.count + needsYou.child })}</span
			>
		{/if}
		{#if headline}<span class="act-detail">{headline}</span>{/if}
	</span>
{/if}

<style>
	.labels {
		display: contents;
	}
	.title {
		display: inline-flex;
		flex: 0 1 auto;
		min-width: 0;
	}
	.title.row {
		flex: 1 1 auto;
		min-width: 8ch;
		font-size: var(--fs-md);
	}
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
	.act-cadence {
		flex: none;
		font-variant-numeric: tabular-nums;
	}
	/* Faint = every task pending/done, accent = one is in_progress, so the row
	   distinguishes "has a list" from "actively working a step" at a glance. */
	.act-todos {
		flex: none;
		font-variant-numeric: tabular-nums;
	}
	.act-todos.running {
		color: var(--accent);
		font-weight: 600;
	}
	/* Never faint: an unanswered request to the human is the one thing on this
	   row that must not read as decoration. */
	.act-needs {
		flex: none;
		color: var(--text);
		font-variant-numeric: tabular-nums;
	}
	.act-needs.blocking {
		color: var(--warn);
		font-weight: 600;
	}
	/* `act-detail` carries arbitrary-length agent prose (the in_progress
	   activeForm). It must stay capped and ellipsized: an unbounded string here
	   grows the lead row and wraps the session emoji onto a second line. */
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
