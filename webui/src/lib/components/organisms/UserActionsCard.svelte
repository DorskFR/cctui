<script lang="ts">
	import { Badge, Card, Checkbox, Heading, Text } from '@dorsk/tsumikit';
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

	const groups = $derived(groupUserActions(actions));

	function kindLabel(kind: UserAction['kind']): string {
		if (kind === 'input') return m.user_actions_kind_input();
		if (kind === 'decision') return m.user_actions_kind_decision();
		return m.user_actions_kind_action();
	}
</script>

{#if groups}
	<Card tone="attention" padding="sm" gap="var(--sp-2)" style="margin:var(--sp-2) 0">
		<div class="head">
			<Heading level={3} size="sm">{m.user_actions_heading()}</Heading>
			{#if groups.blocking > 0}
				<Badge tone="warn" size="xs">{m.user_actions_blocking_badge()}</Badge>
			{/if}
			<span class="count"><Text tone="faint" size="xs">{groups.open.length}</Text></span>
		</div>
		<ul class="rows">
			{#each groups.open as a (a.id)}
				<li class="row" class:blocking={a.blocking}>
					<div class="line">
						{#if interactive}
							<Checkbox label={a.title} checked={false} onchange={() => ontick(a.id, 'done')} />
						{:else}
							<Text size="sm">{a.title}</Text>
						{/if}
						<Badge tone="neutral" size="xs">{kindLabel(a.kind)}</Badge>
					</div>
					{#if a.detail}
						<details>
							<summary><Text tone="faint" size="xs">{m.user_actions_detail()}</Text></summary>
							<div class="detail">{@html renderMarkdown(a.detail)}</div>
						</details>
					{/if}
				</li>
			{/each}
		</ul>
		{#if groups.resolved.length > 0}
			<details>
				<summary>
					<Text tone="faint" size="xs"
						>{m.user_actions_resolved_count({ count: groups.resolved.length })}</Text
					>
				</summary>
				<ul class="rows">
					{#each groups.resolved as a (a.id)}
						<li class="row">
							<div class="line struck">
								<Text size="xs" tone="faint">{a.title}</Text>
								{#if a.note}<Text size="xs" tone="faint">— {a.note}</Text>{/if}
							</div>
						</li>
					{/each}
				</ul>
			</details>
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
		gap: var(--sp-1);
		margin: 0;
		padding: 0;
		list-style: none;
		min-width: 0;
	}
	.row {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}
	.line {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	/* The one thing that must read differently from the rest: the agent is
	   stopped until this item is answered. */
	.row.blocking {
		border-inline-start: 2px solid var(--warn);
		padding-inline-start: var(--sp-2);
	}
	.line.struck {
		text-decoration: line-through;
	}
	summary {
		cursor: pointer;
		list-style: none;
	}
	.detail {
		font-size: var(--fs-xs);
		color: var(--text-muted);
		padding-inline-start: var(--sp-3);
		overflow-wrap: anywhere;
	}
</style>
