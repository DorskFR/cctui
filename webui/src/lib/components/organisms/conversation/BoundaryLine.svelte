<script lang="ts">
	// Context boundaries in the transcript: a `/clear` or `/compact` reset
	// divider, or the summary block a `/compact` produced.
	import type { Line } from './types';
	import { m } from '$lib/paraglide/messages';

	let { ln }: { ln: Line } = $props();
</script>

{#if ln.role === 'reset'}
	<div class="reset-divider" role="separator">
		<span class="reset-chip">⟳ {ln.text}</span>
	</div>
{:else if ln.role === 'compact'}
	<div class="compact-block">
		<div class="compact-head">{m.conversation_context_compacted()}</div>
		{#if ln.html}<div class="compact-body">{@html ln.html}</div>{/if}
	</div>
{/if}

<style>
	/* Context-reset boundary (/clear or /compact) — a full-width rule with
	   a centered chip in its own blue hue. */
	.reset-divider {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
		margin: var(--sp-3) 0;
		color: var(--role-boundary);
	}
	.reset-divider::before,
	.reset-divider::after {
		content: '';
		flex: 1;
		height: 1px;
		background: color-mix(in srgb, var(--role-boundary) 40%, transparent);
	}
	.reset-chip {
		padding: 2px var(--sp-3);
		border-radius: var(--r-pill, 999px);
		border: 1px solid color-mix(in srgb, var(--role-boundary) 45%, transparent);
		background: color-mix(in srgb, var(--role-boundary) 12%, var(--bg-elevated));
		font-size: var(--fs-xs);
		font-weight: var(--fw-medium);
		text-transform: uppercase;
		letter-spacing: 0.04em;
		white-space: nowrap;
	}
	/* Compact-summary block (/compact) — its own blue hue, a filled
	   left-bordered block (not the thin reset divider) so the two boundary kinds
	   read differently. */
	.compact-block {
		margin: var(--sp-3) 0;
		padding: var(--sp-2) var(--sp-3);
		border-left: 3px solid var(--role-boundary);
		border-radius: var(--r-2, 6px);
		background: color-mix(in srgb, var(--role-boundary) 10%, var(--bg-elevated));
	}
	.compact-head {
		color: var(--role-boundary);
		font-size: var(--fs-xs);
		font-weight: var(--fw-medium);
		text-transform: uppercase;
		letter-spacing: 0.04em;
		margin-bottom: var(--sp-1);
	}
	.compact-body {
		font-size: var(--fs-sm);
		opacity: 0.9;
	}
</style>
