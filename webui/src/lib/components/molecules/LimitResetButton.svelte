<script lang="ts">
	// The provider's limit-reset credit as one head-row action: a retry glyph
	// that asks before spending the credit, greyed with the reason when none
	// is available. Reads the same cached usage query as the bars below it.
	import { useAccountUsage, useLimitReset } from '$lib/queries';
	import { Button, IconButton, Modal, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { toasts } from '$lib/toast.svelte';
	import { errMessage } from '$lib/api';
	import { limitResetHint, limitResetLabel } from './limit-reset';

	let { providerId, enabled = true }: { providerId: string; enabled?: boolean } = $props();

	const q = useAccountUsage(
		() => providerId,
		() => enabled
	);
	const reset = $derived(q.data?.limit_reset ?? null);
	const claim = useLimitReset();
	let claiming = $state(false);
	let confirming = $state(false);

	async function onreset() {
		if (!reset || claiming) return;
		confirming = false;
		claiming = true;
		try {
			const r = await claim(providerId, reset.credit_id);
			const text = m.sessions_limit_reset_outcome({ outcome: r.outcome });
			if (r.outcome === 'reset') toasts.ok(text);
			else toasts.error(text);
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			claiming = false;
		}
	}
</script>

{#if reset}
	<IconButton
		icon="retry"
		inline
		size={14}
		label={limitResetLabel(reset)}
		title={reset.available ? limitResetLabel(reset) : limitResetHint(reset)}
		disabled={!reset.available || claiming}
		loading={claiming}
		onclick={() => (confirming = true)}
	/>
	{#if confirming}
		<Modal
			title={m.sessions_limit_reset_confirm_title()}
			tone="warn"
			size="sm"
			onclose={() => (confirming = false)}
		>
			{#snippet body()}
				<Text>{m.sessions_limit_reset_confirm_body({ title: reset.title ?? m.sessions_limit_reset() })}</Text>
			{/snippet}
			{#snippet footer()}
				<Button variant="ghost" onclick={() => (confirming = false)}>{m.sessions_limit_reset_cancel()}</Button>
				<Button tone="warn" onclick={onreset}>{m.sessions_limit_reset_confirm()}</Button>
			{/snippet}
		</Modal>
	{/if}
{/if}
