<script lang="ts">
	import { Button, Modal, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { copyText } from '$lib/clipboard';

	let { title, secret, onclose }: { title: string; secret: string; onclose: () => void } =
		$props();

	async function copy() {
		await copyText(secret, m.users_secret_copied());
	}
</script>

<Modal {title} {onclose}>
	{#snippet body()}
		<div class="stack">
			<Text as="p" tone="muted">{m.users_secret_warning()}</Text>
			<div class="secret-box"><Text variant="code" size="sm" tone="accent" wrap="anywhere" block>{secret}</Text></div>
		</div>
	{/snippet}
	{#snippet footer()}
		<Button block onclick={onclose}>{m.common_close()}</Button>
		<Button block variant="primary" onclick={copy}>{m.common_copy()}</Button>
	{/snippet}
</Modal>

<style>
	.secret-box {
		padding: var(--sp-3);
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
	}
</style>
