<script lang="ts">
	import { Disclosure, Text } from '@dorsk/tsumikit';
	import type { TodoItem, TodoProgress, TodoStatus } from './types';
	import { taskPanelOpen, setTaskPanelOpen } from './taskPanel';
	import { m } from '$lib/paraglide/messages';

	let { sessionId, progress }: { sessionId: string; progress: TodoProgress | null } = $props();

	// Writable $derived: local toggles stick, but switching sessions re-seeds from
	// that session's own persisted state instead of carrying the previous one's.
	let open = $derived(taskPanelOpen(sessionId));

	let expanded = $state<ReadonlySet<string>>(new Set());
	const setExpanded = (key: string, v: boolean) => {
		const next = new Set(expanded);
		if (v) next.add(key);
		else next.delete(key);
		expanded = next;
	};

	const ROW_DISCLOSURE =
		'flex:1;min-width:0;--disclosure-pad:0;--disclosure-panel-pad:0 0 var(--sp-1) calc(1em + var(--sp-2))';

	const GLYPH: Record<TodoStatus, string> = { completed: '✔', in_progress: '▸', pending: '·' };
	const label: Record<TodoStatus, () => string> = {
		completed: m.tasks_status_completed,
		in_progress: m.tasks_status_in_progress,
		pending: m.tasks_status_pending
	};
</script>

{#snippet row(t: TodoItem)}
	<span class="glyph" aria-hidden="true">{GLYPH[t.status]}</span>
	{#if t.id}<Text tone="faint" numeric nowrap>#{t.id}</Text>{/if}
	<span class="subject" title={t.content}>{t.content}</span>
	{#if t.owner}<Text tone="faint" nowrap truncate title={t.owner}>@{t.owner}</Text>{/if}
	<span class="status">{label[t.status]()}</span>
	{#if t.blockedBy?.length}
		<span class="blocked" title={t.blockedBy.join(', ')}>
			{m.tasks_blocked_by({ tasks: t.blockedBy.join(', ') })}
		</span>
	{/if}
{/snippet}

{#if progress}
	<div class="tasks">
		<Disclosure
			chevron="start"
			{open}
			onchange={(v) => {
				open = v;
				setTaskPanelOpen(sessionId, v);
			}}
		>
			{#snippet header()}
				<span class="strip-text">
					<span class="title">{m.tasks_heading()}</span>
					<span class="count">{progress.done}/{progress.total}</span>
					{#if progress.inProgress}
						<span class="now" title={progress.inProgress.activeForm ?? progress.inProgress.content}>
							{progress.inProgress.activeForm ?? progress.inProgress.content}
						</span>
					{/if}
				</span>
			{/snippet}
			{#if open}
				<ul class="list">
					{#each progress.items as t, i (i)}
						<li class="task" class:done={t.status === 'completed'} class:active={t.status === 'in_progress'}>
							{#if t.description}
								{@const key = t.id ?? String(i)}
								<Disclosure
									size="compact"
									chevron="end"
									style={ROW_DISCLOSURE}
									open={expanded.has(key)}
									onchange={(v) => setExpanded(key, v)}
								>
									{#snippet header()}
										<span class="row">{@render row(t)}</span>
									{/snippet}
									{#if expanded.has(key)}
										<Text as="p" tone="faint" size="xs" wrap="anywhere">{t.description}</Text>
									{/if}
								</Disclosure>
							{:else}
								{@render row(t)}
							{/if}
						</li>
					{/each}
				</ul>
			{/if}
		</Disclosure>
	</div>
{/if}

<style>
	/* `flex: none` plus the per-span truncation keeps a long subject or a long
	   activeForm from widening the drawer or wrapping the strip onto a 2nd line. */
	.tasks {
		flex: none;
		min-width: 0;
		max-width: 100%;
		border-bottom: 1px solid var(--border-subtle, var(--border));
		font-size: var(--fs-xs);
		overflow: hidden;
	}
	.strip-text {
		display: flex;
		align-items: baseline;
		gap: var(--sp-2);
		min-width: 0;
		color: var(--text-muted);
		font-size: var(--fs-xs);
		font-weight: var(--fw-normal);
	}
	.title {
		flex: none;
	}
	.count {
		flex: none;
		color: var(--text-faint);
		font-variant-numeric: tabular-nums;
	}
	.now {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--text-faint);
	}
	.list {
		display: flex;
		flex-direction: column;
		gap: 1px;
		margin: 0;
		padding: 0 var(--sp-2) var(--sp-2) var(--sp-2);
		list-style: none;
		min-width: 0;
		max-height: 32vh;
		overflow-y: auto;
		overflow-x: hidden;
	}
	.task {
		display: flex;
		align-items: baseline;
		gap: var(--sp-2);
		min-width: 0;
		color: var(--text-muted);
	}
	.glyph {
		flex: none;
		width: 1em;
		text-align: center;
		color: var(--text-faint);
	}
	.subject {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.row {
		flex: 1;
		display: flex;
		align-items: baseline;
		gap: var(--sp-2);
		min-width: 0;
		text-align: start;
	}
	.status,
	.blocked {
		flex: none;
		max-width: 12rem;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--text-faint);
	}
	.task.done .subject {
		text-decoration: line-through;
		color: var(--text-faint);
	}
	.task.active {
		color: var(--text);
		font-weight: 600;
	}
	.task.active .glyph {
		color: var(--accent, var(--text));
	}
</style>
