<script lang="ts">
	import { Badge, Button, Card, Checkbox, Disclosure, Heading, Icon, Text } from '@dorsk/tsumikit';
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

	const groups = $derived(groupUserActions(actions));
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
</script>

{#snippet title(a: UserAction)}
	<span class="ua-title" data-ua-title>
		{#if a.blocking}<Icon name="alert-circle" tone="warn" label={m.user_actions_blocking_badge()} />{/if}
		<Text size="sm" weight={a.blocking ? 'semibold' : undefined}>{a.title}</Text>
	</span>
{/snippet}

{#snippet item(a: UserAction)}
	<li class="ua-item" class:ua-blocking={a.blocking} data-ua-row data-blocking={a.blocking || undefined}>
		{#if interactive}
			<span class="ua-check">
				<Checkbox label={a.title} labelHidden checked={false} onchange={() => ontick(a.id, 'done')} />
			</span>
		{/if}
		<div class="ua-body">
			{#if a.detail}
				<Disclosure
					chevron="end"
					style="--disclosure-pad: 0; --disclosure-panel-pad: var(--sp-1) 0 0; --disclosure-fs: var(--fs-sm)"
				>
					{#snippet header()}{@render title(a)}{/snippet}
					<div class="ua-detail">{@html renderMarkdown(a.detail ?? '')}</div>
				</Disclosure>
			{:else}
				{@render title(a)}
			{/if}
		</div>
	</li>
{/snippet}

{#if groups}
	<Card padding="sm" gap="var(--sp-2)" style="margin:var(--sp-2) 0">
		<div class="ua-head">
			<Heading level={3} size="sm">{m.user_actions_heading()}</Heading>
			<Badge tone="neutral" size="xs">{groups.open.length}</Badge>
			{#if groups.blocking > 0}
				<Badge tone="warn" size="xs">{m.user_actions_blocking_badge()}</Badge>
			{/if}
		</div>
		{#if groups.blocking > 0}
			<ul class="ua-list ua-blocking-group" aria-label={m.user_actions_blocking_badge()}>
				{#each visible.filter((a) => a.blocking) as a (a.id)}
					{@render item(a)}
				{/each}
			</ul>
		{/if}
		{#if visible.some((a) => !a.blocking)}
			<ul class="ua-list">
				{#each visible.filter((a) => !a.blocking) as a (a.id)}
					{@render item(a)}
				{/each}
			</ul>
		{/if}
		{#if groups.open.length > Math.max(VISIBLE_OPEN, groups.blocking)}
			<div class="ua-more">
				<Button variant="link" size="sm" onclick={() => (showAll = !showAll)}>
					{showAll ? m.user_actions_show_less() : m.user_actions_show_more({ count: hidden })}
				</Button>
			</div>
		{/if}
		{#if groups.resolved.length > 0}
			<Disclosure size="compact" chevron="start" style="--disclosure-pad: var(--sp-1) 0; --disclosure-panel-pad: 0">
				{#snippet header()}
					<Text tone="faint" size="xs">{resolvedSummary}</Text>
				{/snippet}
				<ul class="ua-list ua-resolved">
					{#each groups.resolved as a (a.id)}
						<li class="ua-item ua-resolved-row" class:ua-dropped={a.status === 'dropped'}>
							<span class="ua-mark">
								{#if a.status === 'done'}
									<Icon name="check-circle" tone="ok" label={m.user_actions_status_done()} />
								{:else}
									<Icon name="x-circle" label={m.user_actions_status_dropped()} />
								{/if}
							</span>
							<span class="ua-resolved-text">
								<span class="ua-resolved-title"><Text size="xs" tone="faint">{a.title}</Text></span>
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
	.ua-head {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	.ua-list {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		margin: 0;
		padding: 0;
		list-style: none;
		min-width: 0;
		text-align: start;
	}
	.ua-item {
		display: flex;
		align-items: flex-start;
		gap: var(--sp-2);
		min-width: 0;
		padding: var(--sp-1) var(--sp-2);
		border-inline-start: 3px solid transparent;
		border-radius: var(--r-sm);
	}
	.ua-item.ua-blocking {
		border-inline-start-color: var(--warn);
		background: color-mix(in srgb, var(--warn) 12%, transparent);
	}
	.ua-check {
		flex: none;
		display: inline-flex;
		align-items: center;
		min-height: calc(var(--fs-sm) * 1.5);
	}
	.ua-body {
		flex: 1 1 auto;
		min-width: 0;
	}
	.ua-title {
		display: inline-flex;
		align-items: baseline;
		gap: var(--sp-1);
		min-width: 0;
		text-align: start;
		overflow-wrap: anywhere;
	}
	.ua-detail {
		font-size: var(--fs-xs);
		color: var(--text-muted);
		text-align: start;
		overflow-wrap: anywhere;
		min-width: 0;
	}
	.ua-more {
		display: flex;
		justify-content: flex-start;
		padding-inline-start: var(--sp-2);
	}
	.ua-resolved {
		padding-top: var(--sp-1);
	}
	.ua-resolved-row {
		align-items: baseline;
		padding-block: 0;
	}
	.ua-mark {
		flex: none;
		display: inline-flex;
		font-size: var(--fs-xs);
		color: var(--text-faint);
	}
	.ua-resolved-text {
		display: flex;
		flex-wrap: wrap;
		column-gap: var(--sp-2);
		min-width: 0;
	}
	.ua-resolved-title {
		text-decoration: line-through;
	}
	.ua-resolved-row.ua-dropped .ua-resolved-title {
		text-decoration: none;
		opacity: 0.7;
	}
</style>
