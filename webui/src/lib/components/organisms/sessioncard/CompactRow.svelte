<script lang="ts">
	import { Badge, IconButton, Text, Timestamp } from '@dorsk/tsumikit';
	import StatText from '$lib/components/atoms/StatText.svelte';
	import AccountBadge from '$lib/components/molecules/AccountBadge.svelte';
	import LabelBadge from '$lib/components/molecules/LabelBadge.svelte';
	import { tokenUsageLayout } from '$lib/components/molecules/TokenUsage.logic';
	import { m } from '$lib/paraglide/messages';
	import { statusBadgeTone } from '$lib/format';
	import { sessionEndTitle } from '$lib/sessionEnd';
	import { settings } from '$lib/settings.svelte';
	import { accountTrafficWarning } from '../../../../routes/sessions/sessions.logic';
	import Badges from './Badges.svelte';
	import DraftActions from './DraftActions.svelte';
	import Lead from './Lead.svelte';
	import { metaParts, needsYouOf, statusLabel, type SessionActions, type SessionView } from './view';

	// Line 1: gutter · dot · name (takes the width) · trail (labels · status ·
	// counts · tokens · time, pin and label picker on hover). Line 2: machine ·
	// project · branch · model in muted sans, then the last message. On a narrow
	// row the trail moves to line 2 so the name keeps the whole first line.
	let {
		view,
		actions,
		hue = null
	}: { view: SessionView; actions: SessionActions; hue?: number | null } = $props();
	const s = $derived(view.s);
	const meta = $derived(metaParts(s, { showMachine: view.showMachine, branch: view.branch }).join(' · '));
	const needsYou = $derived(needsYouOf(s));
	const tokens = $derived(
		tokenUsageLayout(s.token_usage, { sum: view.rollup ? view.rollup.tokens : null }).total
	);
	const pinnable = $derived(!view.child && !actions.selectable && !!actions.onTogglePin);
	const pinLabel = $derived(s.pinned ? m.sessions_unpin_aria() : m.sessions_pin_aria());
	const preview = $derived(s.match_snippet ?? view.lastMsg);

	function pin(e: Event) {
		e.stopPropagation();
		actions.onTogglePin?.(s);
	}
</script>

<div class="crow">
	<div class="line1">
		<Lead {view} {actions} row />
		<span class="brk" aria-hidden="true"></span>
		<span class="trail">
		{#if s.labels.length > 0 || actions.labelEditable}
			<span class="labels" class:hover-act={s.labels.length === 0}
				><LabelBadge
					labels={s.labels}
					editable={actions.labelEditable}
					allLabels={actions.allLabels}
					onCreate={actions.onCreateLabel}
					onAttach={(lid) => actions.onAttachLabel?.(s.id, lid)}
					onDetach={(lid) => actions.onDetachLabel?.(s.id, lid)}
					onUpdate={actions.onUpdateLabel}
					onDelete={actions.onDeleteLabel}
				/></span
			>
		{/if}
		{#if needsYou}
			<span
				class="needs"
				class:blocking={needsYou.blocking}
				title={needsYou.child > 0
					? m.sessions_needs_you_child_title({ count: needsYou.child })
					: m.sessions_needs_you_title()}
				>{m.sessions_needs_you({ count: needsYou.count + needsYou.child })}</span
			>
		{/if}
		{#if view.showStatusBadge}<Badge tone={statusBadgeTone(s.status)} size="xs" style="flex:none"
				>{statusLabel(s.status)}</Badge
			>{/if}
		{#if view.end}<span class="end-badge" class:end-muted={view.end.muted} title={sessionEndTitle(view.end)}
				><Badge tone={view.end.tone} size="xs">{view.end.badge}</Badge></span
			>{/if}
		<Badges {view} compact />
		{#if pinnable}
			<span class="pin" class:hover-act={!s.pinned}>
				<IconButton
					icon="star"
					filled={s.pinned}
					variant="ghost"
					inline
					size={14}
					label={pinLabel}
					title={s.pinned ? m.sessions_unpin_title() : m.sessions_pin_title()}
					pressed={s.pinned}
					onpointerdown={(e) => e.stopPropagation()}
					onclick={pin}
				/>
			</span>
		{/if}
		{#if view.draft}
			<DraftActions {view} {actions} />
		{:else if tokens > 0}
			<span class="tokens"><StatText value={tokens} kind="tokens" compact /></span>
		{/if}
		{#if s.last_message_at}<span class="time"
				><Timestamp value={s.last_message_at} mode="relative" tone="faint" size="xs" /></span
			>{/if}
		</span>
	</div>
	{#if meta || preview || hue != null}
		<div class="line2">
			{#if hue != null}<span class="hue" style:--mh={hue} aria-hidden="true"></span>{/if}
			{#if meta}<span class="meta"><Text size="xs" tone="muted" truncate>{meta}</Text></span>{/if}
			{#if preview}
				<span class="preview"
					><Text size="xs" tone={s.match_snippet ? 'default' : 'faint'} truncate>{preview}</Text></span
				>
			{/if}
			<span class="account"
				><AccountBadge
					name={s.account_name}
					warn={accountTrafficWarning(s)}
					showName={settings.accountNames}
				/></span
			>
		</div>
	{/if}
</div>

<style>
	.crow {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		min-width: 0;
	}
	.line1,
	.line2 {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	.trail {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		flex: 0 1 auto;
		min-width: 0;
	}
	.brk {
		display: none;
	}
	.hue {
		flex: none;
		width: 8px;
		height: 8px;
		border-radius: 2px;
		background: hsl(var(--mh) 65% 55%);
	}
	.labels {
		display: inline-flex;
		flex: 0 100 auto;
		min-width: 0;
		overflow: hidden;
	}
	.needs {
		flex: none;
		font-size: var(--fs-xs);
		color: var(--text);
		font-variant-numeric: tabular-nums;
		white-space: nowrap;
	}
	.needs.blocking {
		color: var(--status-attention);
		font-weight: 600;
	}
	.end-badge {
		display: inline-flex;
		flex: 0 50 auto;
		min-width: 0;
		max-width: 22ch;
		overflow: hidden;
	}
	.end-muted {
		opacity: 0.6;
	}
	.pin,
	.tokens,
	.time {
		display: inline-flex;
		flex: none;
		white-space: nowrap;
	}
	.time {
		min-width: 30px;
		justify-content: flex-end;
	}
	.meta {
		display: inline-flex;
		flex: 0 1 auto;
		min-width: 0;
	}
	.preview {
		display: inline-flex;
		flex: 1 1 0;
		min-width: 0;
	}
	.account {
		display: inline-flex;
		flex: none;
		margin-inline-start: auto;
	}
	@media (hover: hover) and (pointer: fine) {
		.hover-act {
			visibility: hidden;
		}
		.crow:hover .hover-act,
		.crow:focus-within .hover-act {
			visibility: visible;
		}
	}
	@container sess-row (max-width: 36.25em) {
		.preview {
			display: none;
		}
		.crow {
			flex-flow: row wrap;
			align-items: center;
			gap: var(--sp-1) var(--sp-2);
		}
		.line1,
		.line2 {
			display: contents;
		}
		.brk {
			display: block;
			flex: 0 0 100%;
			height: 0;
			order: 1;
		}
		.hue,
		.meta {
			order: 2;
		}
		.meta {
			flex: 1 1 0;
		}
		.trail {
			order: 3;
			flex: none;
		}
		.account {
			order: 4;
			margin-inline-start: 0;
		}
	}
	@container sess-row (max-width: 26em) {
		.tokens {
			display: none;
		}
	}
</style>
