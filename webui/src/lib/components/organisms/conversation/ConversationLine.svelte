<script lang="ts">
	// A single rendered conversation message (assistant/user/system/tool/result),
	// extracted from ConversationDrawer. Pure presentation: it renders the meta
	// row (role badge, tool name, time, delivery state), the bubble, and the
	// per-message action buttons, delegating retry/edit/save/copy to callbacks.
	import TokenUsage from '$lib/components/molecules/TokenUsage.svelte';
	import { Badge, Button, IconButton, Text, Timestamp, Tooltip } from '@dorsk/tsumikit';
	import TurnSummaryFooter from './TurnSummaryFooter.svelte';
	import UserAttachments from './UserAttachments.svelte';
	import { parseUserUploadRefs } from './lines';
	import type { Line } from './types';
	import { m } from '$lib/paraglide/messages';
	import './bubble.css';

	let {
		ln,
		archived,
		onretry,
		onedit,
		onsaveimage,
		oncopymarkdown,
		forkable = false,
		selectMode = false,
		selectedForFork = false,
		ontoggleselect
	}: {
		ln: Line;
		archived: boolean;
		onretry: (ts: number) => void;
		onedit: (text: string, ts: number) => void;
		onsaveimage: (e: MouseEvent, ln: Line) => void;
		oncopymarkdown: (ln: Line) => void;
		// Subset-fork affordances: only assistant lines carry the
		// `messageId` anchor shared by the line and the on-disk transcript.
		forkable?: boolean;
		selectMode?: boolean;
		selectedForFork?: boolean;
		onforkfrom?: (messageId: string) => void;
		onforkafter?: (messageId: string) => void;
		ontoggleselect?: (messageId: string) => void;
	} = $props();

	const uploadRefs = $derived(ln.role === 'user' ? parseUserUploadRefs(ln.text) : null);

	const forkAnchor = $derived(
		forkable && ln.role === 'assistant' && ln.messageId ? ln.messageId : null
	);

	function durationLabel(ms: number | undefined): string {
		if (!ms || ms < 1000) return '';
		const secs = Math.round(ms / 1000);
		if (secs < 60) return `${secs}s`;
		const mins = Math.floor(secs / 60);
		return `${mins}m ${secs % 60}s`;
	}

	// Thinking runs long; clamp it and offer a toggle, but only once the content
	// actually overflows the clamp. Measuring while expanded would report no
	// overflow and take the "show less" control away, so skip it then.
	let thinkingEl = $state<HTMLElement>();
	let thinkingExpanded = $state(false);
	let thinkingOverflows = $state(false);
	$effect(() => {
		void ln.html;
		if (thinkingExpanded || !thinkingEl) return;
		thinkingOverflows = thinkingEl.scrollHeight > thinkingEl.clientHeight + 1;
	});
</script>

<div
	class="line {ln.role}"
	data-journey="line"
	data-journey-key={ln.role}
	class:mcp={ln.mcp}
	class:pending={ln.pending}
	class:failed={!!ln.failed}
>
	<div class="lmeta row">
		{#if selectMode && forkAnchor}
			<input
				type="checkbox"
				class="fork-select-check"
				checked={selectedForFork}
				aria-label={m.fork_select_message_aria()}
				title={m.fork_select_message_title()}
				onchange={() => ontoggleselect?.(forkAnchor)}
			/>
		{/if}
		{#if ln.role === 'assistant' && ln.turn !== undefined}
			<Tooltip text={`turn ${ln.turn}`}>
				{#snippet trigger()}<Badge size="xs" uppercase color="var(--bc)">{ln.role}</Badge>{/snippet}
			</Tooltip>
		{:else}
			<Badge size="xs" uppercase color="var(--bc)"
				>{ln.mcp ? 'mcp' : ln.role === 'result' ? 'result' : ln.role}</Badge
			>
		{/if}
		{#if ln.role === 'tool' || ln.role === 'result'}
			<span class="who tool-name">{ln.role === 'result' ? '↳ ' : ''}{ln.tool ?? 'tool'}</span>
		{/if}
		<Timestamp value={ln.ts} mode="time" tone="faint" size="xs" />
		{#if ln.failed}
			<Text class="not-delivered" tone="danger" size="xs" title={ln.failed}>{m.conversation_not_delivered()}</Text>
			{#if !archived}
				<Button
					variant="ghost"
					class="retry-failed"
					title={m.conversation_resend_title({ reason: ln.failed })}
					onclick={() => onretry(ln.ts)}>↻ {m.common_retry()}</Button>
				<IconButton
					class="edit-pending"
					icon="edit"

					label={m.conversation_edit_message_label()}
					title={m.conversation_edit_message_title()}
					onclick={() => onedit(ln.text ?? '', ln.ts)}
				/>
			{/if}
		{:else if ln.pending}
			{#if ln.retrying}
				<Text class="sending" tone="inherit" size="xs" title={m.conversation_retrying_title()}
					>{m.conversation_retrying({ attempt: ln.retrying.attempt, max: ln.retrying.max })}</Text
				>
			{:else}
				<Text class="sending" tone="inherit" size="xs">{m.conversation_sending()}</Text>
			{/if}
			{#if !archived}
				<IconButton
					class="edit-pending"
					icon="edit"

					label={m.conversation_edit_pending_label()}
					title={m.conversation_edit_pending_title()}
					onclick={() => onedit(ln.text ?? '', ln.ts)}
				/>
			{/if}
		{/if}
		<span class="line-actions">
			<!-- Copy-as-Markdown uses the same markdown glyph as the
			     conversation-level copy; save-as-image uses a
			     plain image icon and sits right next to it. -->
			<IconButton
				class="copy"
				icon="image"

				label={m.conversation_save_image_label()}
				title={m.conversation_save_image_title()}
				onclick={(e) => onsaveimage(e, ln)}
			/>
			<IconButton
				class="copy"
				icon="markdown"

				label={m.conversation_copy_markdown_label()}
				title={m.conversation_copy_markdown_title()}
				onclick={() => oncopymarkdown(ln)}
			/>
		</span>
	</div>
	{#if ln.role === 'thinking'}
		<div
			class="bubble think"
			class:redacted={ln.redacted}
			class:clamped={!thinkingExpanded}
			bind:this={thinkingEl}
		>
			{@html ln.html}
		</div>
		{#if thinkingOverflows}
			<button
				type="button"
				class="think-toggle"
				aria-expanded={thinkingExpanded}
				onclick={() => (thinkingExpanded = !thinkingExpanded)}
			>
				{thinkingExpanded ? m.conversation_show_less() : m.conversation_show_more()}
			</button>
		{/if}
	{:else if ln.html}
		<div class="bubble">{@html ln.html}</div>
	{:else if ln.htmlCode}
		<pre class="bubble mono code">{@html ln.htmlCode}</pre>
	{:else}
		<pre class="bubble mono code">{ln.text}</pre>
	{/if}
	{#if uploadRefs && uploadRefs.names.length}
		<UserAttachments refs={uploadRefs} ts={ln.ts} {archived} />
	{/if}
	{#if ln.summary}
		<TurnSummaryFooter summary={ln.summary} />
	{/if}
	{#if (ln.durationMs || ln.usage) && (ln.role === 'assistant' || ln.role === 'result')}
		{@const dur = durationLabel(ln.durationMs)}
		<div class="line-foot row">
			<!-- How long the model took to reply (CCT) — kept alongside the per-reply
			     token breakdown (no Σ; that's the conversation-wide aggregate). -->
			{#if dur}<Text tone="faint" size="xs">⏱ {dur}</Text>{/if}
			{#if ln.usage}<TokenUsage usage={ln.usage} showSum={false} />{/if}
		</div>
	{/if}
</div>

<style>
	.line {
		display: flex;
		flex-direction: column;
		gap: 2px;
		max-width: 100%;
		/* Role tint, inherited by the badge below. Set on the line (not on the
		   badge itself) so one :global rule serves every role. */
		--bc: var(--text-muted);
	}
	.line.user {
		--bc: var(--role-user);
	}
	.line.assistant {
		--bc: var(--role-assistant);
	}
	.line.thinking {
		--bc: var(--role-thinking);
	}
	.line.system {
		--bc: var(--role-system);
	}
	.line.marker {
		--bc: var(--text-faint);
	}
	.line.tool,
	.line.result {
		--bc: var(--role-tool);
	}
	.line.mcp {
		--bc: var(--role-mcp);
	}
	.lmeta {
		gap: var(--sp-2);
		font-size: var(--fs-xs);
		color: var(--text-faint);
	}
	.who {
		text-transform: uppercase;
		letter-spacing: 0.04em;
		font-weight: var(--fw-medium);
	}
	.tool-name {
		font-family: var(--font-mono);
		color: var(--text-muted);
		text-transform: none;
		letter-spacing: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		max-width: 60%;
	}
	/* Role badge pill — rides on the tsumikit Badge atom (pill
	   shape, sizing); these overrides add the per-role tint via --role-* tokens
	   and the uppercase treatment Badge doesn't carry. */
	/* Per-message action buttons (copy-as-Markdown + save-image), pushed to
	   the right of the meta row. Excluded from the saved image. */
	.line-actions {
		margin-left: auto;
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
	}
	.line-actions :global(.copy) {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		padding: var(--sp-1);
		min-width: auto;
		min-height: auto;
		line-height: 1;
		color: var(--text-muted);
	}
	.line-actions :global(.copy:hover) {
		color: var(--text);
	}
	.line-actions :global(.copy svg) {
		width: 1rem;
		height: 1rem;
	}
	/* Layout only; typography (faint xs) is the Text atom's. */
	.line .line-foot {
		align-self: flex-end;
		padding-inline: var(--sp-1);
	}
	/* Uniform role tints — all via --role-* tokens. */
	.line.user .bubble {
		background: color-mix(in srgb, var(--role-user) 14%, var(--bg-elevated));
		border-color: color-mix(in srgb, var(--role-user) 45%, transparent);
	}
	.line.assistant .bubble {
		border-left: 2px solid color-mix(in srgb, var(--role-assistant) 55%, transparent);
	}
	/* System/agent-directed messages (harness wake-ups, task notifications,
	   injected reminders) — purple, distinct from the green user bubbles so
	   they don't read as something the human typed. */
	.line.system .bubble {
		background: color-mix(in srgb, var(--role-system) 12%, var(--bg-elevated));
		border-color: color-mix(in srgb, var(--role-system) 40%, transparent);
	}
	/* Harness bookkeeping (permission-mode flips, worktree/title updates) —
	   deliberately the quietest bubble in the log. */
	.line.marker .bubble {
		background: none;
		border-color: var(--border);
		color: var(--text-faint);
		font-size: var(--fs-xs);
	}
	/* Optimistic reply: muted/amber until the agent acknowledges, then it
	   settles into the regular green user tint above. */
	.line.user.pending .bubble {
		background: color-mix(in srgb, var(--warn) 10%, var(--bg-elevated));
		border-color: color-mix(in srgb, var(--warn) 35%, transparent);
		opacity: 0.85;
	}
	/* Amber tint + push-right; rides on a Text child, so :global. */
	.lmeta :global(.sending) {
		color: var(--warn);
		margin-left: auto;
	}
	/* Edit-pending button: sits next to "sending…" on a pending line. */
	.lmeta :global(.edit-pending) {
		padding: 0 var(--sp-1);
		min-width: auto;
		min-height: auto;
		font-size: var(--fs-sm);
		line-height: 1;
		color: var(--text-faint);
	}
	.lmeta :global(.edit-pending:hover) {
		color: var(--accent);
	}
	/* Failed send: the bubble goes red and a Retry control appears. */
	.line.user.failed .bubble {
		background: color-mix(in srgb, var(--danger) 12%, var(--bg-elevated));
		border-color: color-mix(in srgb, var(--danger) 50%, transparent);
	}
	/* Push-right + nowrap; colour (danger) is the Text atom's. Rides on a Text
	   child, so :global. */
	.lmeta :global(.not-delivered) {
		margin-left: auto;
		white-space: nowrap;
	}
	.lmeta :global(.retry-failed) {
		padding: 0 var(--sp-1);
		min-height: auto;
		font-size: var(--fs-sm);
		line-height: 1;
		color: var(--danger);
		font-weight: 600;
	}
	.lmeta :global(.retry-failed:hover) {
		color: color-mix(in srgb, var(--danger) 70%, var(--text));
	}
	.line.tool .bubble,
	.line.result .bubble {
		background: var(--bg-elevated-2);
		border-left: 2px solid color-mix(in srgb, var(--role-tool) 55%, transparent);
	}
	.line.tool.mcp .bubble {
		border-left-color: color-mix(in srgb, var(--role-mcp) 60%, transparent);
	}
	/* Reasoning — muted brown, visually behind the prose it produced. */
	.line.thinking .bubble.think {
		background: color-mix(in srgb, var(--role-thinking) 10%, var(--bg-elevated));
		border-color: color-mix(in srgb, var(--role-thinking) 35%, transparent);
		border-left: 2px solid color-mix(in srgb, var(--role-thinking) 60%, transparent);
		color: color-mix(in srgb, var(--role-thinking) 45%, var(--md-text));
	}
	.line.thinking .bubble.think.clamped {
		max-height: 12rem;
		overflow: hidden;
		/* Fade the cut edge so a clamped block reads as truncated, not as ended. */
		mask-image: linear-gradient(to bottom, #000 8rem, transparent);
	}
	/* Provider withheld the content; only the placeholder remains. */
	.line.thinking .bubble.think.redacted {
		font-style: italic;
		opacity: 0.7;
	}
	.think-toggle {
		align-self: flex-start;
		margin-top: 2px;
		padding: 0 var(--sp-1);
		background: none;
		border: none;
		color: var(--role-thinking);
		font-size: var(--fs-xs);
		font-weight: var(--fw-medium);
		cursor: pointer;
	}
	.think-toggle:hover {
		text-decoration: underline;
	}
	.code {
		white-space: pre-wrap;
		max-height: 22rem;
		overflow: auto;
		font-size: calc(var(--fs-sm) - 0.0625rem);
	}
</style>
