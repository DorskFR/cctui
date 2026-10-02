<script lang="ts" module>
	import type { Line } from './types';

	/** The role colour a badge next to the bubble takes as `--bc`. */
	export function roleColor(role: Line['role'] | string, mcp = false): string {
		if (mcp) return 'var(--role-mcp)';
		switch (role) {
			case 'user':
			case 'assistant':
			case 'thinking':
			case 'system':
			case 'peer':
			case 'poll':
				return `var(--role-${role})`;
			case 'tool':
			case 'result':
				return 'var(--role-tool)';
			case 'marker':
				return 'var(--text-faint)';
			default:
				return 'var(--text-muted)';
		}
	}
</script>

<script lang="ts">
	// A message body as the conversation drawer draws it: rendered markdown, or
	// highlighted code for tool calls and results, tinted by role.
	import './bubble.css';

	let {
		role,
		mcp = false,
		html,
		htmlCode,
		text,
		tinted = false,
		pending = false,
		queued = false,
		failed = false,
		cancelled = false
	}: {
		role: Line['role'] | string;
		mcp?: boolean;
		html?: string;
		htmlCode?: string;
		text?: string;
		/** Settings › Sessions "role-tinted background". */
		tinted?: boolean;
		pending?: boolean;
		queued?: boolean;
		failed?: boolean;
		cancelled?: boolean;
	} = $props();
</script>

{#if html}
	<div class="bubble {role}" class:mcp class:tinted class:pending class:queued class:failed class:cancelled>{@html html}</div>
{:else if htmlCode}
	<pre class="bubble mono code {role}" class:mcp class:tinted class:pending class:queued class:failed class:cancelled>{@html htmlCode}</pre>
{:else if text}
	<pre class="bubble mono code {role}" class:mcp class:tinted class:pending class:queued class:failed class:cancelled>{text}</pre>
{/if}

<style>
	/* Opt-in (Settings › Sessions): the whole bubble background takes the
	   role colour, on top of the rails below. Mixed into --bg-elevated so it
	   follows light and dark themes alike; user/system go a step stronger
	   than their always-on tint so they still stand apart. */
	.bubble.tinted.assistant {
		background: color-mix(in srgb, var(--role-assistant) 11%, var(--bg-elevated));
	}
	.bubble.tinted.tool,
	.bubble.tinted.result {
		background: color-mix(in srgb, var(--role-tool) 11%, var(--bg-elevated));
	}
	.bubble.tinted.mcp {
		background: color-mix(in srgb, var(--role-mcp) 11%, var(--bg-elevated));
	}
	.bubble.tinted.user {
		background: color-mix(in srgb, var(--role-user) 22%, var(--bg-elevated));
	}
	.bubble.tinted.system {
		background: color-mix(in srgb, var(--role-system) 20%, var(--bg-elevated));
	}
	/* Uniform role tints — all via --role-* tokens. */
	.bubble.user {
		background: color-mix(in srgb, var(--role-user) 14%, var(--bg-elevated));
		border-color: color-mix(in srgb, var(--role-user) 45%, transparent);
	}
	.bubble.assistant {
		border-left: 2px solid color-mix(in srgb, var(--role-assistant) 55%, transparent);
	}
	/* System/agent-directed messages (harness wake-ups, task notifications,
	   injected reminders) — purple, distinct from the green user bubbles so
	   they don't read as something the human typed. */
	.bubble.system {
		background: color-mix(in srgb, var(--role-system) 12%, var(--bg-elevated));
		border-color: color-mix(in srgb, var(--role-system) 40%, transparent);
	}
	.bubble.peer {
		background: color-mix(in srgb, var(--role-peer) 12%, var(--bg-elevated));
		border-color: color-mix(in srgb, var(--role-peer) 40%, transparent);
	}
	.bubble.poll {
		background: color-mix(in srgb, var(--role-poll) 12%, var(--bg-elevated));
		border-color: color-mix(in srgb, var(--role-poll) 40%, transparent);
	}
	/* Optimistic reply: muted/amber until the agent acknowledges, then it
	   settles into the regular green user tint above. */
	.bubble.user.pending {
		background: color-mix(in srgb, var(--warn) 10%, var(--bg-elevated));
		border-color: color-mix(in srgb, var(--warn) 35%, transparent);
		opacity: 0.85;
	}
	.bubble.user.queued {
		background: color-mix(in srgb, var(--role-queued) 12%, var(--bg-elevated));
		border-color: color-mix(in srgb, var(--role-queued) 40%, transparent);
	}
	.bubble.user.cancelled {
		opacity: 0.6;
		text-decoration: line-through;
	}
	/* Failed send: the bubble goes red and a Retry control appears. */
	.bubble.user.failed {
		background: color-mix(in srgb, var(--danger) 12%, var(--bg-elevated));
		border-color: color-mix(in srgb, var(--danger) 50%, transparent);
	}
	.bubble.tool,
	.bubble.result {
		background: var(--bg-elevated-2);
		border-left: 2px solid color-mix(in srgb, var(--role-tool) 55%, transparent);
	}
	.bubble.tool.mcp {
		border-left-color: color-mix(in srgb, var(--role-mcp) 60%, transparent);
	}
	.code {
		white-space: pre-wrap;
		max-height: 22rem;
		overflow: auto;
		font-size: calc(var(--fs-sm) - 0.0625rem);
	}
</style>
