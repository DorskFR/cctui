<script lang="ts">
	import { Badge, Button, Card, Checkbox, Disclosure, Heading, Icon, IconButton, Text } from '@dorsk/tsumikit';
	import { SvelteSet } from 'svelte/reactivity';
	import { m } from '$lib/paraglide/messages';
	import { renderMarkdown } from '$lib/markdown';
	import type { UserAction } from '@bindings/UserAction';
	import type { UserActionStatus } from '@bindings/UserActionStatus';
	import { groupUserActions } from './conversation/userActions';

	let {
		actions,
		interactive,
		ontick
	}: {
		actions: UserAction[];
		/** False for an archived or ended session: the list stays readable, ticks stop. */
		interactive: boolean;
		ontick: (id: string, status: UserActionStatus) => void;
	} = $props();

	/** Open rows shown before "show N more"; blocking rows are never folded. */
	const VISIBLE_OPEN = 8;

	const uid = $props.id();
	const groups = $derived(groupUserActions(actions));
	const openDetails = new SvelteSet<string>();
	let showAll = $state(false);

	const visible = $derived.by(() => {
		if (!groups) return [];
		if (showAll) return groups.open;
		return groups.open.slice(0, Math.max(VISIBLE_OPEN, groups.blocking));
	});
	const hidden = $derived(groups ? groups.open.length - visible.length : 0);
	const done = $derived(groups ? groups.resolved.filter((a) => a.status === 'done').length : 0);
	const dropped = $derived(groups ? groups.resolved.length - done : 0);
	const resolvedSummary = $derived(
		[
			done > 0 ? m.user_actions_done_count({ count: done }) : '',
			dropped > 0 ? m.user_actions_dropped_count({ count: dropped }) : ''
		]
			.filter(Boolean)
			.join(' · ')
	);

	function toggleDetail(id: string) {
		if (openDetails.has(id)) openDetails.delete(id);
		else openDetails.add(id);
	}
</script>

{#snippet row(a: UserAction)}
	{@const expanded = openDetails.has(a.id)}
	{@const panelId = `${uid}-detail-${a.id}`}
	<li class="row" class:blocking={a.blocking}>
		<div class="line">
			{#if interactive}
				<Checkbox label={a.title} checked={false} onchange={() => ontick(a.id, 'done')} />
			{:else}
				<Text size="sm">{a.title}</Text>
			{/if}
			{#if a.detail}
				<span class="toggle">
					<IconButton
						icon="chevron-down"
						label={m.user_actions_detail()}
						variant="ghost"
						box="xs"
						hitArea="compact"
						aria-expanded={expanded}
						aria-controls={panelId}
						style={expanded ? 'transform: rotate(180deg)' : ''}
						onclick={() => toggleDetail(a.id)}
					/>
				</span>
			{/if}
		</div>
		{#if a.detail && expanded}
			<div class="detail" class:indent={interactive} id={panelId}>
				{@html renderMarkdown(a.detail)}
			</div>
		{/if}
	</li>
{/snippet}

{#if groups}
	<Card tone="attention" padding="sm" gap="var(--sp-2)" style="margin:var(--sp-2) 0">
		<div class="head">
			<Heading level={3} size="sm">{m.user_actions_heading()}</Heading>
			{#if groups.blocking > 0}
				<Badge tone="warn" size="xs">{m.user_actions_blocking_badge()}</Badge>
			{/if}
			<span class="count"><Text tone="faint" size="xs">{groups.open.length}</Text></span>
		</div>
		{#if groups.blocking > 0}
			<ul class="rows blocking-group" aria-label={m.user_actions_blocking_badge()}>
				{#each visible.filter((a) => a.blocking) as a (a.id)}
					{@render row(a)}
				{/each}
			</ul>
		{/if}
		{#if visible.some((a) => !a.blocking)}
			<ul class="rows">
				{#each visible.filter((a) => !a.blocking) as a (a.id)}
					{@render row(a)}
				{/each}
			</ul>
		{/if}
		{#if groups.open.length > Math.max(VISIBLE_OPEN, groups.blocking)}
			<div class="more">
				<Button variant="ghost" size="sm" onclick={() => (showAll = !showAll)}>
					{showAll ? m.user_actions_show_less() : m.user_actions_show_more({ count: hidden })}
				</Button>
			</div>
		{/if}
		{#if groups.resolved.length > 0}
			<Disclosure size="compact" chevron="start" style="--disclosure-pad: var(--sp-1) 0; --disclosure-panel-pad: 0">
				{#snippet header()}
					<Text tone="faint" size="xs">{resolvedSummary}</Text>
				{/snippet}
				<ul class="rows resolved">
					{#each groups.resolved as a (a.id)}
						<li class="row resolved-row" class:dropped={a.status === 'dropped'}>
							<span class="mark">
								{#if a.status === 'done'}
									<Icon name="check-circle" tone="ok" label={m.user_actions_status_done()} />
								{:else}
									<Icon name="x-circle" label={m.user_actions_status_dropped()} />
								{/if}
							</span>
							<span class="resolved-text">
								<span class="resolved-title"><Text size="xs" tone="faint">{a.title}</Text></span>
								{#if a.note}<Text size="xs" tone="faint" italic>{a.note}</Text>{/if}
							</span>
						</li>
					{/each}
				</ul>
			</Disclosure>
		{/if}
	</Card>
{/if}

<style>
	.head {
		display: flex;
		align-items: baseline;
		gap: var(--sp-2);
		min-width: 0;
	}
	.count {
		margin-inline-start: auto;
		font-variant-numeric: tabular-nums;
	}
	.rows {
		display: flex;
		flex-direction: column;
		align-items: stretch;
		gap: var(--sp-1);
		margin: 0;
		padding: 0;
		list-style: none;
		min-width: 0;
		text-align: start;
	}
	.row {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		min-width: 0;
	}
	.line {
		display: flex;
		align-items: flex-start;
		justify-content: flex-start;
		gap: var(--sp-1);
		min-width: 0;
	}
	.line > :first-child {
		flex: 1 1 auto;
		min-width: 0;
	}
	.toggle {
		flex: none;
		display: inline-flex;
	}
	.blocking-group {
		padding: var(--sp-2);
		border-inline-start: 3px solid var(--warn);
		border-radius: var(--r-md);
		background: color-mix(in srgb, var(--warn) 10%, transparent);
	}
	.row.blocking {
		font-weight: var(--fw-semibold);
	}
	.detail {
		font-size: var(--fs-xs);
		color: var(--text-muted);
		text-align: start;
		overflow-wrap: anywhere;
		min-width: 0;
	}
	.detail.indent {
		padding-inline-start: calc(var(--cb-box, 1.15rem) + var(--sp-2));
	}
	.more {
		display: flex;
		justify-content: flex-start;
	}
	.resolved {
		padding-top: var(--sp-1);
	}
	.resolved-row {
		flex-direction: row;
		align-items: baseline;
		gap: var(--sp-2);
	}
	.mark {
		flex: none;
		display: inline-flex;
		font-size: var(--fs-xs);
		color: var(--text-faint);
	}
	.resolved-text {
		display: flex;
		flex-wrap: wrap;
		column-gap: var(--sp-2);
		min-width: 0;
	}
	.resolved-title {
		text-decoration: line-through;
	}
	.resolved-row.dropped .resolved-title {
		text-decoration: none;
		opacity: 0.7;
	}
</style>
