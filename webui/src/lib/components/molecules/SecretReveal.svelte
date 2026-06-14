<script lang="ts">
	import { Button, Modal, Text } from '@dorsk/tsumikit';
	import { toasts } from '$lib/toast.svelte';

	let { title, secret, onclose }: { title: string; secret: string; onclose: () => void } =
		$props();

	async function copy() {
		try {
			await navigator.clipboard.writeText(secret);
			toasts.ok('Copied to clipboard');
		} catch {
			toasts.err('Clipboard unavailable');
		}
	}
</script>

<Modal {title} {onclose}>
	{#snippet body()}
		<div class="stack">
			<Text as="p" tone="muted">Copy this now — it is shown only once and cannot be retrieved later.</Text>
			<Text variant="code" size="sm" tone="accent" class="secret">{secret}</Text>
		</div>
	{/snippet}
	{#snippet footer()}
		<Button block onclick={onclose}>Done</Button>
		<Button block variant="primary" onclick={copy}>Copy</Button>
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
