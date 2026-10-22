<script lang="ts">
	import { Button, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import './bubble.css';

	let {
		html,
		redacted,
		noReply = false
	}: { html: string | undefined; redacted: boolean | undefined; noReply?: boolean } = $props();

	// Thinking runs long; clamp it and offer a toggle, but only once the content
	// actually overflows the clamp. Measuring while expanded would report no
	// overflow and take the "show less" control away, so skip it then.
	let el = $state<HTMLElement>();
	let expanded = $state(false);
	let overflows = $state(false);
	$effect(() => {
		void html;
		if (expanded || !el) return;
		overflows = el.scrollHeight > el.clientHeight + 1;
	});
</script>

<div class="bubble think" class:redacted class:clamped={!expanded} bind:this={el}>
	{@html html}
</div>
{#if overflows}
	<Button
		variant="link"
		size="sm"
		shrink={false}
		style="margin-top:2px;color:var(--role-thinking)"
		aria-expanded={expanded}
		onclick={() => (expanded = !expanded)}
	>
		{expanded ? m.conversation_show_less() : m.conversation_show_more()}
	</Button>
{/if}
{#if noReply}
	<div class="no-reply" role="note">
		<Text tone="faint" size="xs" italic>{m.conversation_thinking_no_reply()}</Text>
	</div>
{/if}

<style>
	/* Reasoning — muted brown, visually behind the prose it produced. */
	.bubble.think {
		background: color-mix(in srgb, var(--role-thinking) 10%, var(--bg-elevated));
		border-color: color-mix(in srgb, var(--role-thinking) 35%, transparent);
		border-left: 2px solid color-mix(in srgb, var(--role-thinking) 60%, transparent);
		color: color-mix(in srgb, var(--role-thinking) 45%, var(--md-text));
	}
	.bubble.think.clamped {
		max-height: 12rem;
		overflow: hidden;
		/* Fade the cut edge so a clamped block reads as truncated, not as ended. */
		mask-image: linear-gradient(to bottom, #000 8rem, transparent);
	}
	.no-reply {
		padding-inline: var(--sp-1);
	}
	/* Provider withheld the content; only the placeholder remains. */
	.bubble.think.redacted {
		font-style: italic;
		opacity: 0.7;
	}
</style>
