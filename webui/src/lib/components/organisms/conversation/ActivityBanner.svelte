<script lang="ts">
	import { now as clockNow } from '#lib/clock.svelte.ts';
	import type { ConversationStream } from './stream.svelte';
	import { formatElapsed } from './activity';
	import { m } from '#lib/paraglide/messages.js';

	let {
		stream,
		archived = false,
		needsInput = false,
		waitingSince = null
	}: {
		stream: ConversationStream;
		archived?: boolean;
		needsInput?: boolean;
		waitingSince?: number | null;
	} = $props();

	// The agent is blocked on the user while an ask/plan/permission is pending —
	// same gating the inline spinner this replaces used.
	const active = $derived(
		stream.working && !archived && !stream.ask && !stream.plan && stream.perms.length === 0
	);

	const waiting = $derived(!active && needsInput);

	// Ticks only while something is counting, so an idle drawer costs no timer.
	const now = $derived(
		active ? clockNow(1_000) : waiting && waitingSince !== null ? clockNow(60_000) : Date.now()
	);
	const idleLabel = $derived(
		!waiting
			? m.conversation_activity_idle()
			: waitingSince === null
				? m.conversation_activity_waiting()
				: m.conversation_activity_waiting_age({ age: waitAge(now - waitingSince) })
	);

	function waitAge(ms: number): string {
		const mins = Math.max(0, Math.floor(ms / 60_000));
		return mins < 1 ? '<1m' : mins < 60 ? `${mins}m` : `${Math.floor(mins / 60)}h ${mins % 60}m`;
	}

	const tool = $derived(active ? stream.currentTool : null);
	const progress = $derived(stream.todoProgress);
	// Priority: the in_progress task's gerund, else the running tool, else the
	// last assistant line — so the banner is never blank mid-turn.
	const status = $derived(
		progress?.inProgress?.activeForm ?? tool?.tool ?? stream.lastAssistantLine ?? m.conversation_working()
	);
	const turnElapsed = $derived(stream.turnStartedAt === null ? null : formatElapsed(now - stream.turnStartedAt));
	const toolElapsed = $derived(tool ? formatElapsed(now - tool.startedAt) : null);
	const tokens = $derived(stream.turnTokensIn + stream.turnTokensOut);
</script>

{#if !archived}
	<div
		class="activity"
		class:idle={!active && !waiting}
		class:waiting
		role="status"
		aria-live="polite"
		data-journey="activity"
		data-journey-key={active ? 'live' : waiting ? 'waiting' : 'idle'}
	>
		<div class="row">
			<span class="dot" class:spin={active} aria-hidden="true"></span>
			<span class="status" title={active ? status : undefined}>
				{active ? status : idleLabel}
			</span>
			{#if active}
				<span class="meta">
					{#if turnElapsed}<span>{turnElapsed}</span>{/if}
					{#if tokens > 0}<span>↓{stream.turnTokensIn} ↑{stream.turnTokensOut}</span>{/if}
					{#if progress}<span>{progress.done}/{progress.total}</span>{/if}
				</span>
			{/if}
		</div>
		{#if tool && tool.summary}
			<div class="row sub">
				<span class="branch" aria-hidden="true">└</span>
				<span class="status" title={tool.summary}>{tool.summary}</span>
				{#if toolElapsed}<span class="meta"><span>{toolElapsed}</span></span>{/if}
			</div>
		{/if}
	</div>
{/if}

<style>
	/* `flex: none` + the per-span truncation below is what keeps a long command
	   or activeForm from growing the banner and pushing the composer. */
	.activity {
		flex: none;
		display: flex;
		flex-direction: column;
		gap: 1px;
		padding: var(--sp-1) var(--sp-2);
		border-top: 1px solid var(--border-subtle, var(--border));
		font-size: var(--fs-xs);
		color: var(--text-muted);
		overflow: hidden;
	}
	.activity.idle {
		color: var(--text-faint);
	}
	.activity.waiting {
		color: var(--warn);
	}
	.waiting .dot {
		background: var(--warn);
	}
	.row {
		display: flex;
		align-items: baseline;
		gap: var(--sp-2);
		min-width: 0;
		max-width: 100%;
	}
	.status {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.sub .status {
		font-family: var(--font-mono, monospace);
		color: var(--text-faint);
	}
	.meta {
		flex: none;
		display: flex;
		gap: var(--sp-2);
		color: var(--text-faint);
		font-variant-numeric: tabular-nums;
		white-space: nowrap;
	}
	.branch {
		flex: none;
		color: var(--text-faint);
	}
	.dot {
		flex: none;
		width: 6px;
		height: 6px;
		border-radius: 50%;
		background: var(--text-faint);
	}
	.dot.spin {
		background: var(--accent, var(--text));
		animation: pulse 1.2s ease-in-out infinite;
	}
	@keyframes pulse {
		0%,
		100% {
			opacity: 0.35;
		}
		50% {
			opacity: 1;
		}
	}
	@media (prefers-reduced-motion: reduce) {
		.dot.spin {
			animation: none;
		}
	}
</style>
