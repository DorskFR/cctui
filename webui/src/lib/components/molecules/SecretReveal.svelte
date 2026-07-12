<script lang="ts">
	import { Button, Modal, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { toasts } from '$lib/toast.svelte';

	let { title, secret, onclose }: { title: string; secret: string; onclose: () => void } =
		$props();

	async function copy() {
		try {
			await navigator.clipboard.writeText(secret);
			toasts.ok(m.users_secret_copied());
		} catch {
			toasts.err(m.users_secret_clipboard_unavailable());
		}
	}
</script>

<Modal {title} {onclose}>
	{#snippet body()}
		<div class="stack">
			<Text as="p" tone="muted">{m.users_secret_warning()}</Text>
			<Text variant="code" size="sm" tone="accent" class="secret">{secret}</Text>
		</div>
	{/snippet}
	{#snippet footer()}
		<Button block onclick={onclose}>{m.users_secret_done()}</Button>
		<Button block variant="primary" onclick={copy}>{m.common_copy()}</Button>
	{/snippet}
</Modal>

<style>
	/* .secret is rendered by the Text atom (which owns its size/tone/mono), so
	   the residual box chrome must be :global to reach that element. */
	:global(.secret) {
		display: block;
		padding: var(--sp-3);
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		word-break: break-all;
	}
</style>
