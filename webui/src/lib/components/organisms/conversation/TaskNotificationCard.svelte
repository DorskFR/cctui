<script lang="ts">
	import { Badge, Button, Text } from '@dorsk/tsumikit';
	import { copyText } from '$lib/clipboard';
	import { taskTone, type TaskNotification } from './format';
	import { m } from '$lib/paraglide/messages';

	let { note }: { note: TaskNotification } = $props();

	const TONE_COLOR = {
		success: 'var(--ok)',
		danger: 'var(--danger)',
		neutral: 'var(--text-muted)'
	} as const;

	const color = $derived(TONE_COLOR[taskTone(note.status)]);
</script>

<div class="bubble task">
	<div class="head">
		{#if note.status}
			<Badge size="xs" uppercase {color}>{note.status}</Badge>
		{/if}
		<Text size="sm" weight="semibold">{note.summary ?? m.conversation_task_notification()}</Text>
	</div>
	{#if note.outputFile}
		<div class="out">
			<div class="path"><Text as="div" variant="code" size="xs" truncate>{note.outputFile}</Text></div>
			<Button
				size="sm"
				title={m.conversation_task_output_copy()}
				onclick={() => copyText(note.outputFile ?? '', m.conversation_task_output_copied())}
			>
				{m.common_copy()}
			</Button>
		</div>
	{/if}
	{#if note.taskId}
		<Text as="div" tone="faint" size="xs" variant="code">{m.conversation_task_id({ id: note.taskId })}</Text>
	{/if}
</div>

<style>
	.task {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		min-width: 0;
	}
	.head {
		display: flex;
		align-items: baseline;
		gap: var(--sp-2);
		min-width: 0;
		flex-wrap: wrap;
	}
	.out {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	/* Owns the shrink that lets the path truncate instead of spilling. */
	.path {
		flex: 1;
		min-width: 0;
		padding: var(--sp-1) var(--sp-2);
		background: var(--bg);
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
	}
</style>
