<script lang="ts">
	// A single rendered conversation message (assistant/user/system/tool/result),
	// extracted from ConversationDrawer. Pure presentation: it renders the meta
	// row (role badge, tool name, time, delivery state), the bubble, and the
	// per-message action buttons, delegating retry/edit/save/copy to callbacks.
	import { clockTime } from '$lib/format';
	import IconButton from '$lib/components/molecules/IconButton.svelte';
	import TokenUsage from '$lib/components/molecules/TokenUsage.svelte';
	import { Button, Text } from '@dorsk/tsumikit';
	import type { Line } from './types';
	import './bubble.css';

	let {
		ln,
		archived,
		tooltip,
		onretry,
		onedit,
		onsaveimage,
		oncopymarkdown
	}: {
		ln: Line;
		archived: boolean;
		// Precomputed hover tooltip for this line's timestamp (CCT-345 / CCT-331).
		tooltip: string;
		onretry: (ts: number) => void;
		onedit: (text: string, ts: number) => void;
		onsaveimage: (e: MouseEvent, ln: Line) => void;
		oncopymarkdown: (ln: Line) => void;
	} = $props();

	function durationLabel(ms: number | undefined): string {
		if (!ms || ms < 1000) return '';
		const secs = Math.round(ms / 1000);
		if (secs < 60) return `${secs}s`;
		const mins = Math.floor(secs / 60);
		return `${mins}m ${secs % 60}s`;
	}
</script>

<div
	class="line {ln.role}"
	class:mcp={ln.mcp}
	class:pending={ln.pending}
	class:failed={!!ln.failed}
>
	<div class="lmeta row">
		<span class="badge-role" class:mcp={ln.mcp}
			>{ln.mcp ? 'mcp' : ln.role === 'result' ? 'result' : ln.role}</span
		>
		{#if ln.role === 'tool' || ln.role === 'result'}
			<span class="who tool-name">{ln.role === 'result' ? '↳ ' : ''}{ln.tool ?? 'tool'}</span>
		{/if}
		<Text tone="faint" size="xs" title={tooltip}>{clockTime(ln.ts)}</Text>
		{#if ln.failed}
			<Text class="not-delivered" tone="danger" size="xs" title={ln.failed}>⚠ Not delivered</Text>
			{#if !archived}
				<Button
					variant="ghost"
					class="retry-failed"
					title="Resend this message ({ln.failed})"
					onclick={() => onretry(ln.ts)}>↻ Retry</Button>
				<IconButton
					class="edit-pending"
					icon="edit"
					label="Edit message"
					title="Pull this message back into the composer to edit and resend"
					onclick={() => onedit(ln.text ?? '', ln.ts)}
				/>
			{/if}
		{:else if ln.pending}
			{#if ln.retrying}
				<Text class="sending" tone="inherit" size="xs" title="Delivery failed — retrying with backoff"
					>retrying… ({ln.retrying.attempt}/{ln.retrying.max})</Text
				>
			{:else}
				<Text class="sending" tone="inherit" size="xs">sending…</Text>
			{/if}
			{#if !archived}
				<IconButton
					class="edit-pending"
					icon="edit"
					label="Edit pending message"
					title="Pull this still-pending message back into the composer to edit and resend"
					onclick={() => onedit(ln.text ?? '', ln.ts)}
				/>
			{/if}
		{/if}
		<span class="line-actions">
			<!-- Copy-as-Markdown uses the same markdown glyph as the
			     conversation-level copy (CCT-301 #5); save-as-image uses a
			     plain image icon and sits right next to it (CCT-301 #1). -->
			<IconButton
				class="copy"
				icon="image"
				label="Save as image"
				title="Save this message as an image"
				onclick={(e) => onsaveimage(e, ln)}
			/>
			<IconButton
				class="copy"
				icon="markdown"
				label="Copy as Markdown"
				title="Copy this message as Markdown"
				onclick={() => oncopymarkdown(ln)}
			/>
		</span>
	</div>
	{#if ln.html}
		<div class="bubble">{@html ln.html}</div>
	{:else if ln.htmlCode}
		<pre class="bubble mono code">{@html ln.htmlCode}</pre>
	{:else}
		<pre class="bubble mono code">{ln.text}</pre>
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
	/* Role badge pill (CCT-161 item 2) — colored per role via --role-* tokens. */
	.badge-role {
		--bc: var(--text-muted);
		display: inline-flex;
		align-items: center;
		padding: 1px var(--sp-2);
		border-radius: var(--r-pill);
		font-size: 0.6875rem;
		font-weight: var(--fw-semibold);
		text-transform: uppercase;
		letter-spacing: 0.04em;
		color: var(--bc);
		background: color-mix(in srgb, var(--bc) 14%, transparent);
		border: 1px solid color-mix(in srgb, var(--bc) 40%, transparent);
		white-space: nowrap;
	}
	.line.user .badge-role {
		--bc: var(--role-user);
	}
	.line.assistant .badge-role {
		--bc: var(--role-assistant);
	}
	.line.system .badge-role {
		--bc: var(--role-system);
	}
	.line.tool .badge-role {
		--bc: var(--role-tool);
	}
	.line.result .badge-role {
		--bc: var(--role-tool);
	}
	.line.tool.mcp .badge-role,
	.badge-role.mcp {
		--bc: var(--role-mcp);
	}
	/* Per-message action buttons (copy-as-Markdown + save-image, CCT-297 #17/#18),
	   pushed to the right of the meta row. Excluded from the saved image. */
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
	/* Layout only; typography (faint xs) is the Text atom's. The class rides on a
	   Text child, so it must be :global to reach it. */
	.line :global(.line-foot) {
		align-self: flex-end;
		padding-inline: var(--sp-1);
	}
	/* Uniform role tints (CCT-161 item 1) — all via --role-* tokens. */
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
	/* Edit-pending button (CCT-208): sits next to "sending…" on a pending line. */
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
	/* Failed send (CCT-212): the bubble goes red and a Retry control appears. */
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
	.code {
		white-space: pre-wrap;
		max-height: 22rem;
		overflow: auto;
		font-size: calc(var(--fs-sm) - 0.0625rem);
	}
</style>
