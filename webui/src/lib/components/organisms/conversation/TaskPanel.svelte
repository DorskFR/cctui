<script lang="ts">
	import type { TodoProgress, TodoStatus } from './types';
	import { taskPanelOpen, setTaskPanelOpen } from './taskPanel';
	import { m } from '$lib/paraglide/messages';

	let { sessionId, progress }: { sessionId: string; progress: TodoProgress | null } = $props();

	// Writable $derived: local toggles stick, but switching sessions re-seeds from
	// that session's own persisted state instead of carrying the previous one's.
	let open = $derived(taskPanelOpen(sessionId));

	function toggle() {
		open = !open;
		setTaskPanelOpen(sessionId, open);
	}

	const GLYPH: Record<TodoStatus, string> = { completed: '✔', in_progress: '▸', pending: '·' };
	const label: Record<TodoStatus, () => string> = {
		completed: m.tasks_status_completed,
		in_progress: m.tasks_status_in_progress,
		pending: m.tasks_status_pending
	};
</script>

{#if progress}
	<div class="tasks" class:open>
		<button type="button" class="strip" onclick={toggle} aria-expanded={open}>
			<span class="chev" aria-hidden="true">{open ? '▾' : '▸'}</span>
			<span class="title">{m.tasks_heading()}</span>
			<span class="count">{progress.done}/{progress.total}</span>
			{#if progress.inProgress}
				<span class="now" title={progress.inProgress.activeForm ?? progress.inProgress.content}>
					{progress.inProgress.activeForm ?? progress.inProgress.content}
				</span>
			{/if}
		</button>
		{#if open}
			<ul class="list">
				{#each progress.items as t, i (i)}
					<li class="task" class:done={t.status === 'completed'} class:active={t.status === 'in_progress'}>
						<span class="glyph" aria-hidden="true">{GLYPH[t.status]}</span>
						<span class="subject" title={t.content}>{t.content}</span>
						<span class="status">{label[t.status]()}</span>
						{#if t.blockedBy?.length}
							<span class="blocked" title={t.blockedBy.join(', ')}>
								{m.tasks_blocked_by({ tasks: t.blockedBy.join(', ') })}
							</span>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}
	</div>
{/if}

<style>
	/* `flex: none` plus the per-span truncation keeps a long subject or a long
	   activeForm from widening the drawer or wrapping the strip onto a 2nd line. */
	.tasks {
		flex: none;
		display: flex;
		flex-direction: column;
		min-width: 0;
		max-width: 100%;
		border-bottom: 1px solid var(--border-subtle, var(--border));
		font-size: var(--fs-xs);
		overflow: hidden;
	}
	.strip {
		display: flex;
		align-items: baseline;
		gap: var(--sp-2);
		width: 100%;
		min-width: 0;
		padding: var(--sp-1) var(--sp-2);
		background: none;
		border: none;
		color: var(--text-muted);
		font-size: inherit;
		text-align: left;
		cursor: pointer;
	}
	.strip:hover {
		background: var(--bg-elevated-2);
	}
	.chev {
		flex: none;
		width: 1em;
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
