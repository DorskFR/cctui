<script lang="ts">
	import Modal from './Modal.svelte';
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
			<p class="muted">Copy this now — it is shown only once and cannot be retrieved later.</p>
			<code class="secret mono">{secret}</code>
		</div>
	{/snippet}
	{#snippet footer()}
		<button class="btn btn-block" onclick={onclose}>Done</button>
		<button class="btn btn-primary btn-block" onclick={copy}>Copy</button>
	{/snippet}
</Modal>

<style>
	.secret {
		display: block;
		padding: var(--sp-3);
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		word-break: break-all;
		font-size: var(--fs-sm);
		color: var(--accent);
	}
</style>
