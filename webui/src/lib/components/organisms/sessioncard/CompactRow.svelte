<script lang="ts" module>
	import { MediaQuery } from 'svelte/reactivity';

	const fineHover = new MediaQuery('(hover: hover) and (pointer: fine)');
</script>

<script lang="ts">
	import { Badge, IconButton, Timestamp } from '@dorsk/tsumikit';
	import AdapterIcon from '#lib/components/atoms/AdapterIcon.svelte';
	import AccountBadge from '#lib/components/molecules/AccountBadge.svelte';
	import LabelBadge from '#lib/components/molecules/LabelBadge.svelte';
	import SessionDot from '#lib/components/molecules/SessionDot.svelte';
	import { tokenUsageLayout } from '#lib/components/molecules/TokenUsage.logic.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { compact, hashHue, machineInitial, modelShort, statusBadgeTone } from '#lib/format.ts';
	import { settings } from '#lib/settings.svelte.ts';
	import { accountTrafficWarning } from '../../../../routes/sessions/sessions.logic';
	import Badges from './Badges.svelte';
	import DraftActions from './DraftActions.svelte';
	import Gutter from './Gutter.svelte';
	import { projectOf, statusLabel, type SessionActions, type SessionView } from './view';

	// Comfortable: a 2-line grid. gutter (star) · line 1 identity · line 1 attention,
	// then line 2 context + preview · line 2 run (model · Σ · provider).
	// Compact density and subagent children collapse to one line.
	let { view, actions }: { view: SessionView; actions: SessionActions } = $props();
	const s = $derived(view.s);
	const live = $derived(view.livenessClass === 'dot-active');
	const oneLine = $derived(view.child || settings.state.sessionList.density === 'compact');
	const pinnable = $derived(!view.child && !actions.selectable && !!actions.onTogglePin);
	const preview = $derived(s.match_snippet ?? view.lastMsg);
	const machine = $derived(s.machine_name || s.machine_id.slice(0, 8));
	const branch = $derived(view.branch && !['main', 'master'].includes(view.branch) ? view.branch : null);
	const model = $derived(
		s.model ? `${modelShort(s.model)}${s.effort && s.effort !== 'medium' ? ` · ${s.effort}` : ''}` : null
	);
	let labelsOpen = $state(false);
	let hovered = $state(false);
	const showAddLabel = $derived(!fineHover.current || hovered || labelsOpen);
	const usage = $derived(
		tokenUsageLayout(s.token_usage, { sum: view.rollup ? view.rollup.tokens : null, cold: s.cache_cold })
	);

	function pin(e: Event) {
		e.stopPropagation();
		actions.onTogglePin?.(s);
	}
</script>

{#snippet tokens()}
	{#if !view.draft && usage.total > 0}
		<span class="tokens" title="Σ {usage.total.toLocaleString()}"
			>{compact(usage.total)}{#if usage.showCold}<span
					class="cold"
					role="img"
					aria-label={m.sessions_token_cold_hint()}
					title={m.sessions_token_cold_hint()}>❄️</span
				>{/if}<span class="prov"><AdapterIcon adapter={s.adapter_id} size={14} /></span></span
		>
	{/if}
{/snippet}

{#snippet time()}
	<span class="time"
		>{#if s.last_message_at}<Timestamp value={s.last_message_at} mode="relative" short tone="faint" size="xs" />{/if}</span
	>
{/snippet}

{#snippet subToggles()}
	{#each actions.subagentToggles as t (t.key)}
		<button
			type="button"
			class="sub"
			aria-expanded={t.open}
			title={m.sessions_subagent_total({ label: t.label, count: t.count }) +
				(t.running > 0 ? m.sessions_subagent_running({ count: t.running }) : '')}
			onpointerdown={(e) => e.stopPropagation()}
			onclick={(e) => {
				e.stopPropagation();
				t.ontoggle();
			}}>{t.count}<span class="sub-word">&nbsp;{t.label}</span></button
		>
	{/each}
{/snippet}

<div
	class="sr"
	role="presentation"
	onpointerenter={() => (hovered = true)}
	onpointerleave={() => (hovered = false)}
	onfocusin={() => (hovered = true)}
	onfocusout={(e) => {
		if (!(e.currentTarget as HTMLElement).contains(e.relatedTarget as Node | null)) hovered = false;
	}}
	class:one={oneLine} class:child={view.child} class:idle={!live} class:stale={view.stale}>
	<span class="gut">
		{#if actions.selectable}
			<Gutter child={false} selectable selected={actions.selected} subagentToggles={[]} />
		{:else if pinnable && !view.child}
			<span class="star" class:on={s.pinned}>
				<IconButton
					icon="star"
					filled={s.pinned}
					variant="ghost"
					inline
					size={14}
					label={s.pinned ? m.sessions_unpin_aria() : m.sessions_pin_aria()}
					title={s.pinned ? m.sessions_unpin_title() : m.sessions_pin_title()}
					pressed={s.pinned}
					onpointerdown={(e) => e.stopPropagation()}
					onclick={pin}
				/>
			</span>
		{/if}
	</span>

	<div class="l1">
		{#if view.child}<span class="arrow" aria-hidden="true">↳</span>{/if}
		<span
			class="slot"
			class:unread={view.unreadCount > 0}
			title={view.unreadCount > 0 ? m.sessions_unread_title({ count: view.unreadCount }) : undefined}
			><SessionDot session={s} livenessClass={view.livenessClass} now={view.now} /></span
		>
		<span class="title" data-journey="title">{view.title}</span>
		{#if !view.child}
			{@render subToggles()}
			{#if actions.showLabels && (s.labels.length > 0 || (actions.labelEditable && showAddLabel))}
				<span class="labels"
					><LabelBadge
						labels={s.labels}
						editable={actions.labelEditable}
						addHidden={!showAddLabel}
						allLabels={actions.allLabels}
						onCreate={actions.onCreateLabel}
						onAttach={(lid) => actions.onAttachLabel?.(s.id, lid)}
						onDetach={(lid) => actions.onDetachLabel?.(s.id, lid)}
						onUpdate={actions.onUpdateLabel}
						onDelete={actions.onDeleteLabel}
						onOpenChange={(o) => (labelsOpen = o)}
					/></span
				>
			{/if}
		{/if}
	</div>

	<div class="m1">
		{#if view.showStatusBadge}<Badge tone={statusBadgeTone(s.status)} size="xs" style="flex:none"
				>{statusLabel(s.status)}</Badge
			>{/if}
		<Badges {view} compact />
		{#if view.draft}<DraftActions {view} {actions} />{/if}
		{#if oneLine && model}<span class="model" title={s.model}>{model}</span>{/if}
		{#if oneLine}{@render tokens()}{/if}
		{@render time()}
	</div>

	<div class="l2">
		{#if !view.child}
			<span class="who">
				<span class="slot" class:named={settings.accountNames}
					><AccountBadge name={s.account_name} warn={accountTrafficWarning(s)} showName={settings.accountNames} /></span
				>
				{#if view.showMachine}<span class="machine" style:--mh={hashHue(machine)} title={machine}
						><span class="m-full">{machine}</span><span class="m-initial">{machineInitial(machine)}</span></span
					>{/if}
			</span>
			<span class="cwd" title={s.working_dir}>{projectOf(s.working_dir)}</span>
			{#if branch}<span class="branch" title={m.sessions_branch_title({ branch })}>⑂ {branch}</span>{/if}
		{/if}
		{#if preview}<span class="preview" class:hit={!!s.match_snippet}>{preview}</span>{/if}
	</div>

	{#if !oneLine}
		<div class="m2">
			{#if model}<span class="model" title={s.model}>{model}</span>{/if}
			{@render tokens()}
		</div>
	{/if}
</div>

<style>
	.sr {
		display: grid;
		grid-template-columns: 14px minmax(0, 1fr) auto;
		column-gap: 6px;
		row-gap: 3px;
		min-width: 0;
		font-size: var(--fs-xs);
	}
	.gut {
		grid-row: 1;
		grid-column: 1;
		align-self: center;
		display: flex;
		justify-content: center;
	}
	.l1 {
		grid-row: 1;
		grid-column: 2;
	}
	.m1 {
		grid-row: 1;
		grid-column: 3;
	}
	.l2 {
		grid-row: 2;
		grid-column: 2;
	}
	.m2 {
		grid-row: 2;
		grid-column: 3;
	}
	.l1,
	.l2 {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	.m1,
	.m2 {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: var(--sp-2);
		white-space: nowrap;
	}

	.title {
		font-size: var(--fs-md);
		font-weight: var(--fw-semibold);
		color: var(--text);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		min-width: 0;
	}
	.sub {
		appearance: none;
		background: none;
		font: inherit;
		font-size: var(--fs-xs);
		font-weight: var(--fw-medium);
		line-height: max(18px, 1.5em);
		white-space: nowrap;
		cursor: pointer;
		flex: none;
		position: relative;
		display: inline-flex;
		align-items: center;
		gap: 4px;
		padding: 0 8px;
		border-radius: 999px;
		border: 1px solid color-mix(in srgb, var(--info) 60%, transparent);
		color: var(--info);
	}
	.sub::before {
		content: '';
		position: absolute;
		inset: -4px;
	}
	.sub:hover {
		background: color-mix(in srgb, var(--info) 16%, transparent);
	}
	.sub[aria-expanded='true'] {
		background: color-mix(in srgb, var(--info) 30%, transparent);
		color: var(--text);
	}
	/* Zero block size: the 24px chips and hover-revealed tag trigger overhang the
	   title line instead of growing the row on hover. */
	.labels {
		display: inline-flex;
		flex: none;
		align-items: center;
		align-self: center;
		height: 0;
	}
	.star {
		display: inline-flex;
		color: var(--text-faint);
	}
	.star.on {
		color: var(--warn);
	}
	.arrow {
		color: var(--text-faint);
		line-height: 1;
	}

	.time {
		flex: none;
		min-width: 3ch;
		text-align: right;
		white-space: nowrap;
	}

	/* Dot and account mark share one centred column so they line up across lines. */
	.slot {
		display: inline-flex;
		justify-content: center;
		width: 16px;
		flex: none;
	}
	.slot.named {
		width: auto;
	}
	.who {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		flex: none;
	}
	.machine {
		font-weight: var(--fw-semibold);
		color: hsl(var(--mh) var(--mach-fg-sl));
		white-space: nowrap;
	}
	.cwd {
		font-family: var(--font-mono);
		color: var(--text-muted);
		white-space: nowrap;
		flex: none;
	}
	.branch {
		font-family: var(--font-mono);
		color: var(--c-violet);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		min-width: 0;
		flex: 0 1 auto;
		max-width: 220px;
	}
	.preview {
		color: var(--text-muted);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		min-width: 0;
		flex: 1 1 0;
	}
	.preview.hit {
		color: var(--text);
	}

	.model {
		color: var(--text-faint);
	}
	.tokens {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		font-family: var(--font-mono);
		color: var(--warn);
	}
	.cold {
		font-size: 0.85em;
		cursor: help;
	}
	.idle .tokens {
		color: var(--text-faint);
	}
	.prov {
		display: inline-flex;
		width: 14px;
		justify-content: center;
	}

	/* One line: compact density and subagent children. */
	.one {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.one .gut {
		width: 14px;
		flex: none;
	}
	.one .l1 {
		flex: 0 1 auto;
		max-width: 46%;
	}
	/* Grid, not flex: lets min-content floor the title at 6em so labels never clip. */
	.one:not(.child) .l1 {
		display: grid;
		grid-auto-flow: column;
		grid-template-columns: auto minmax(6em, max-content);
		grid-auto-columns: max-content;
		min-width: min-content;
	}
	.one .title {
		max-width: 300px;
	}
	.one .l2 {
		flex: 1 1 0;
		min-width: min-content;
	}
	.one .preview {
		width: 0;
	}
	.one .cwd,
	.one .branch {
		display: none;
	}
	.one .l2 {
		order: 1;
	}
	.one .m1 {
		order: 2;
	}
	.stale {
		opacity: 0.6;
	}
	.slot.unread {
		position: relative;
	}
	.slot.unread::before {
		content: '';
		position: absolute;
		top: 50%;
		left: 50%;
		width: 15px;
		height: 15px;
		transform: translate(-50%, -50%);
		border-radius: var(--r-pill);
		border: 1.5px solid var(--accent);
		pointer-events: none;
	}
	.child .l1 {
		padding-left: min(calc((var(--agent-depth, 1) - 1) * var(--sp-3)), 2.5rem);
	}
	.child .title {
		font-size: var(--fs-sm);
		font-weight: var(--fw-medium);
	}

	@media (hover: hover) and (pointer: fine) {
		.star:not(.on) {
			visibility: hidden;
		}
		.sr:hover .star,
		.sr:focus-within .star {
			visibility: visible;
		}
	}
	.m-initial {
		display: none;
	}
	/* Narrow: the preview only fills room left after the title; too little room
	   wraps it onto a clipped second line instead of leaving a lone ellipsis. */
	@container sess-row (max-width: 36.25em) {
		.model,
		.branch,
		.sub-word,
		.m-full {
			display: none;
		}
		.m-initial {
			display: inline;
		}
		.l2 {
			flex-wrap: wrap;
			height: 18px;
			overflow: hidden;
			row-gap: 0;
		}
		.preview {
			width: 0;
			min-width: 4em;
		}
		.one .l1 {
			max-width: none;
		}
		.one .title {
			max-width: none;
		}
	}
	@container sess-row (max-width: 28em) {
		.one .tokens {
			display: none;
		}
	}
	@container sess-row (max-width: 22em) {
		.one .preview {
			display: none;
		}
	}
</style>
