<script lang="ts">
	import { Card, Heading, Text } from '@dorsk/tsumikit';
	import { todoProgress } from './conversation/format';
	import type { TodoItem } from './conversation/types';
	import { m } from '$lib/paraglide/messages';

	let { todos }: { todos: TodoItem[] } = $props();

	const progress = $derived(todoProgress(todos));

	const GLYPH = { completed: '✔', in_progress: '▸', pending: '·' } as const;
</script>

{#if progress}
	<Card tone="neutral" surface="raised" padding="sm" gap="var(--sp-2)" style="margin:var(--sp-2) 0">
		<div class="head">
			<Heading level={3} size="sm">{m.todo_heading()}</Heading>
			<Text tone="faint" size="xs">{progress.done}/{progress.total}</Text>
		</div>
		<ul class="todo-list">
			{#each progress.items as t, i (i)}
				<li class="todo-row" class:done={t.status === 'completed'} class:active={t.status === 'in_progress'}>
					<span class="glyph" aria-hidden="true">{GLYPH[t.status]}</span>
					<span class="label" title={t.content}>{t.content}</span>
				</li>
			{/each}
		</ul>
	</Card>
{/if}

<style>
	.head {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: var(--sp-2);
		min-width: 0;
	}
	.todo-list {
		display: flex;
		flex-direction: column;
		gap: 1px;
		margin: 0;
		padding: 0;
		list-style: none;
		min-width: 0;
	}
	.todo-row {
		display: flex;
		align-items: baseline;
		gap: var(--sp-2);
		min-width: 0;
		font-size: var(--fs-xs);
		color: var(--text-muted);
	}
	.glyph {
		flex: none;
		width: 1em;
		text-align: center;
		color: var(--text-faint);
	}
	.label {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.todo-row.done .label {
		text-decoration: line-through;
		color: var(--text-faint);
	}
	.todo-row.active {
		color: var(--text);
		font-weight: 600;
	}
	.todo-row.active .glyph {
		color: var(--accent, var(--text));
	}
</style>
