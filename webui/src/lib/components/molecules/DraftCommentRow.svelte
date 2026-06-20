<!--
  GH-VIEW-4: an inline draft comment rendered directly under the diff line it
  anchors to. Shows the comment body with edit/delete, or — when `composing` —
  a textarea + Add/Cancel to author a new draft comment INSTANTLY (no GitHub
  round-trip; the draft store persists it the moment Add is pressed).

  Purely presentational: the parent (`DiffViewer`) owns the draft state and the
  query mutations; this only emits intents. Mirrors the ConversationComposer
  Textarea/Button conventions.
-->
<script lang="ts">
	import type { DraftCommentInfo } from '@bindings/DraftCommentInfo';
	import { Button, Cluster, Stack, Text, Textarea } from '@dorsk/tsumikit';

	interface Props {
		/** An existing draft comment to display (omit when `composing`). */
		comment?: DraftCommentInfo;
		/** Render the new-comment composer instead of an existing comment. */
		composing?: boolean;
		/** Whether a mutation is in flight (disables the action buttons). */
		busy?: boolean;
		onsave?: (body: string) => void;
		ondelete?: (commentId: string) => void;
		oncancel?: () => void;
	}
	const { comment, composing = false, busy = false, onsave, ondelete, oncancel }: Props = $props();

	let editing = $state(composing);
	let text = $state(comment?.body ?? '');

	function start() {
		text = comment?.body ?? '';
		editing = true;
	}
	function save() {
		const body = text.trim();
		if (!body) return;
		onsave?.(body);
		if (!composing) editing = false;
	}
	function cancel() {
		editing = false;
		text = comment?.body ?? '';
		oncancel?.();
	}
	function onkeydown(e: KeyboardEvent) {
		// Cmd/Ctrl+Enter submits; Escape cancels — matches the conversation composer.
		if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
			e.preventDefault();
			save();
		} else if (e.key === 'Escape') {
			e.preventDefault();
			cancel();
		}
	}
</script>

<div class="draft-comment">
	{#if editing}
		<Stack gap="var(--sp-1)">
			<Textarea
				bind:value={text}
				rows={3}
				placeholder="Leave a draft comment (Cmd/Ctrl+Enter to add)…"
				{onkeydown}
				autofocus
			/>
			<Cluster gap="var(--sp-2)">
				<Button onclick={save} disabled={busy || !text.trim()}>
					{composing ? 'Add comment' : 'Save'}
				</Button>
				<Button variant="ghost" onclick={cancel} disabled={busy}>Cancel</Button>
			</Cluster>
		</Stack>
	{:else if comment}
		<Stack gap="var(--sp-1)">
			<div class="body"><Text size="sm">{comment.body}</Text></div>
			<Cluster gap="var(--sp-2)" align="center">
				<Text tone="muted" size="xs">draft</Text>
				<button type="button" class="link" onclick={start} disabled={busy}>Edit</button>
				<button
					type="button"
					class="link danger"
					onclick={() => ondelete?.(comment.id)}
					disabled={busy}>Delete</button
				>
			</Cluster>
		</Stack>
	{/if}
</div>

<style>
	.draft-comment {
		padding: var(--sp-2) var(--sp-3);
		margin-left: 8em;
		border-left: 3px solid var(--accent, #4c8bf5);
		background: var(--surface-1, rgba(127, 127, 127, 0.04));
	}
	.body {
		white-space: pre-wrap;
		word-break: break-word;
	}
	.link {
		border: 0;
		background: transparent;
		color: var(--accent, #4c8bf5);
		cursor: pointer;
		font: inherit;
		font-size: 0.75rem;
		padding: 0;
	}
	.link.danger {
		color: var(--syn-danger, #f85149);
	}
	.link:disabled {
		opacity: 0.5;
		cursor: default;
	}
</style>
