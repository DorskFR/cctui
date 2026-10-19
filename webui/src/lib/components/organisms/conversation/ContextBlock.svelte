<script lang="ts">
	import { Disclosure } from '@dorsk/tsumikit';
	import { injectedSummary } from './format';
	import { m } from '$lib/paraglide/messages';
	import './bubble.css';

	let { html, text }: { html: string | undefined; text: string | undefined } = $props();

	const summary = $derived(injectedSummary(text ?? ''));
</script>

<div class="ctx">
	<Disclosure size="compact" chevron="start">
		{#snippet header()}
			<span class="first">{summary.first}</span>
			<span class="count">{m.conversation_injected_lines({ count: summary.lines })}</span>
		{/snippet}
		<div class="bubble context">{@html html}</div>
	</Disclosure>
</div>

<style>
	/* Injected background material: a one-line strip until opened, never a
	   full-size bubble. */
	.ctx {
		border: 1px solid color-mix(in srgb, var(--role-context) 35%, transparent);
		border-left: 2px solid color-mix(in srgb, var(--role-context) 60%, transparent);
		border-radius: var(--r-md);
		background: color-mix(in srgb, var(--role-context) 8%, var(--bg-elevated));
		color: color-mix(in srgb, var(--role-context) 50%, var(--md-text));
		min-width: 0;
	}
	.first {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-weight: var(--fw-normal);
	}
	.count {
		flex: none;
		margin-left: auto;
		color: var(--text-faint);
		font-weight: var(--fw-normal);
	}
	.bubble.context {
		max-height: 32rem;
		overflow: auto;
		background: none;
		border: 0;
		color: inherit;
	}
</style>
