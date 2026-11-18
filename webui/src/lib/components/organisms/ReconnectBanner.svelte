<script lang="ts">
	import { Button, Callout } from '@dorsk/tsumikit';
	import { m } from '#lib/paraglide/messages.js';
	import { now } from '#lib/clock.svelte.ts';
	import { retryCountdown } from '#lib/backendHealth.svelte.ts';

	let {
		nextRetryAt,
		probing,
		onretry
	}: { nextRetryAt: number | null; probing: boolean; onretry: () => void } = $props();

	const seconds = $derived(retryCountdown(nextRetryAt, probing, now()));
	const status = $derived(
		seconds === null ? m.maintenance_retrying() : m.maintenance_next_retry({ seconds })
	);
</script>

<div class="reconnect" data-testid="reconnect-banner">
	<Callout tone="warn" busy={probing} title={m.reconnect_banner_title()}>
		<span class="num">{status}</span>
		{#snippet actions()}
			<Button size="sm" onclick={onretry} disabled={probing}>{m.maintenance_retry_now()}</Button>
		{/snippet}
	</Callout>
</div>

<style>
	.reconnect {
		position: fixed;
		inset-block-start: var(--sp-2);
		inset-inline: 0;
		z-index: var(--z-toast);
		display: flex;
		justify-content: center;
		padding-inline: var(--sp-2);
		pointer-events: none;
	}
	.reconnect > :global(*) {
		pointer-events: auto;
		max-inline-size: 40rem;
	}
</style>
