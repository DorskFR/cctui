<script lang="ts">
	// The first user message, kept reachable from anywhere in the transcript.
	// Sits above the scroll viewport (not inside it), so expanding it shrinks the
	// viewport's clientHeight — a layout-induced scroll ScrollController re-pins.
	import { IconButton, Text } from '@dorsk/tsumikit';
	import { briefSummary } from './brief';
	import type { Line } from './types';
	import { m } from '$lib/paraglide/messages';
	import './bubble.css';

	let {
		line,
		expanded = $bindable(false),
		oncopy,
		onjump
	}: {
		line: Line;
		expanded?: boolean;
		oncopy: (ln: Line) => void;
		/** Omit when the message has no seq to jump to (never happens for a
		 *  server-fetched head, but the head query can be cold). */
		onjump?: (seq: number) => void;
	} = $props();

	const summary = $derived(briefSummary(line));
</script>

<div class="brief" class:expanded>
	<div class="brief-head">
		<span class="role-dot" aria-hidden="true"></span>
		<Text tone="faint" size="xs" class="brief-tag">{m.conversation_brief_label()}</Text>
		<button
			type="button"
			class="brief-toggle"
			aria-expanded={expanded}
			title={expanded ? m.conversation_brief_collapse() : m.conversation_brief_expand()}
			onclick={() => (expanded = !expanded)}
		>
			<span class="brief-summary">{summary}</span>
			<span class="chev" aria-hidden="true">{expanded ? '▾' : '▸'}</span>
		</button>
		{#if onjump && line.seq !== undefined}
			<IconButton
				box="xs"
				hitArea="compact"
				variant="ghost"
				glyphSize="0.9rem"
				icon="link"
				label={m.conversation_brief_jump()}
				title={m.conversation_brief_jump()}
				onclick={() => onjump?.(line.seq as number)}
			/>
		{/if}
		<IconButton
			box="xs"
			hitArea="compact"
			variant="ghost"
			glyphSize="0.9rem"
			icon="markdown"
			label={m.conversation_copy_markdown_label()}
			title={m.conversation_copy_markdown_title()}
			onclick={() => oncopy(line)}
		/>
	</div>
	{#if expanded}
		<div class="brief-body bubble">
			{#if line.html}{@html line.html}{:else}{line.text}{/if}
		</div>
	{/if}
</div>

<style>
	.brief {
		flex: none;
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		padding: var(--sp-1) var(--sp-3);
		border-bottom: 1px solid var(--border);
		background: var(--bg-elevated);
	}
	.brief-head {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	.role-dot {
		flex: none;
		width: 7px;
		height: 7px;
		border-radius: 50%;
		background: var(--role-user);
	}
	.brief-toggle {
		flex: 1;
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
		padding: 0;
		background: none;
		border: none;
		color: var(--text);
		font-size: var(--fs-xs);
		text-align: left;
		cursor: pointer;
	}
	.brief-summary {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.chev {
		flex: none;
		color: var(--text-faint);
	}
	.brief-body {
		max-height: 40vh;
		overflow: auto;
		font-size: var(--fs-sm);
		background: color-mix(in srgb, var(--role-user) 10%, var(--bg-elevated));
		border-color: color-mix(in srgb, var(--role-user) 40%, transparent);
	}
</style>
