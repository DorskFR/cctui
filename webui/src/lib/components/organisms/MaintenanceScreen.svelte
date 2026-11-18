<script lang="ts">
	import { Button, Container, EmptyState, Stack, Text } from '@dorsk/tsumikit';
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

<main class="maintenance" data-testid="maintenance-screen">
	<Container>
		<EmptyState
			loading
			loadingDelay={0}
			title={m.maintenance_title()}
			description={m.maintenance_description()}
		>
			{#snippet action()}
				<Stack align="center">
					<Text tone="muted" class="num" aria-live="polite">{status}</Text>
					<Button onclick={onretry} disabled={probing}>{m.maintenance_retry_now()}</Button>
				</Stack>
			{/snippet}
		</EmptyState>
	</Container>
</main>

<style>
	.maintenance {
		min-height: 100dvh;
		display: grid;
		align-items: center;
		padding-block: var(--sp-6);
	}
</style>
