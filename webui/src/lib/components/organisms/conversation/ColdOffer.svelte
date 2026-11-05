<script lang="ts">
	import { Button, IconButton, Text } from '@dorsk/tsumikit';
	import type { EnterRoute } from '$lib/followup';
	import { m } from '$lib/paraglide/messages';

	let {
		route,
		onsendhere,
		onfollowup,
		ondismiss
	}: {
		route: EnterRoute;
		onsendhere: () => void;
		onfollowup: () => void;
		ondismiss: () => void;
	} = $props();
</script>

<div class="cold-offer" data-testid="cold-offer">
	<Text tone="muted" size="sm">
		{#if route === 'launch'}
			{m.composer_followup_auto_offer()}
			<Button variant="link" size="sm" onclick={onsendhere}>{m.composer_followup_send_here_link()}</Button>
		{:else}
			{m.composer_followup_offer()}
			<Button variant="link" size="sm" onclick={onfollowup}>{m.composer_followup_start()}</Button>
		{/if}
	</Text>
	<IconButton icon="x" variant="ghost" box="xs" label={m.composer_followup_dismiss()} onclick={ondismiss} />
</div>

<style>
	.cold-offer {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
	}
</style>
