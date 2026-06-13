<script lang="ts">
	import { auth } from '$lib/auth.svelte';
	import { apiBase } from '$lib/config';
	import Button from '$lib/components/atoms/Button.svelte';
	import Card from '$lib/components/atoms/Card.svelte';
	import Field from '$lib/components/molecules/Field.svelte';
	import Input from '$lib/components/atoms/Input.svelte';
	import Text from '$lib/components/atoms/Text.svelte';

	let token = $state('');
	let err = $state('');
	let busy = $state(false);

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		if (!token.trim() || busy) return;
		busy = true;
		err = '';
		try {
			// Validate the token against an admin-only read before persisting it.
			const res = await fetch(`${apiBase()}/admin/users`, {
				headers: { Authorization: `Bearer ${token.trim()}` }
			});
			if (res.status === 401 || res.status === 403) {
				err = 'Invalid or unauthorized token.';
			} else if (!res.ok) {
				err = `Server error (${res.status}).`;
			} else {
				auth.set(token.trim());
			}
		} catch {
			err = 'Could not reach the server.';
		} finally {
			busy = false;
		}
	}
</script>

<div class="login">
	<Card as="form" class="stack" style="width:100%;max-width:22rem" onsubmit={submit}>
		<Text size="xl" weight="bold" class="brand"><Text variant="code" tone="accent">»_</Text> cctui</Text>
		<Text as="p" tone="muted">Enter an admin or user token to continue.</Text>
		<Field label="Token" for="token">
			<Input
				id="token"
				mono
				type="password"
				autocomplete="current-password"
				placeholder="cctui token"
				bind:value={token}
			/>
		</Field>
		{#if err}<Text as="div" tone="danger" size="sm">{err}</Text>{/if}
		<Button variant="primary" block type="submit" disabled={busy || !token.trim()}>
			{#if busy}<span class="spin"></span>{:else}Sign in{/if}
		</Button>
	</Card>
</div>

<style>
	.login {
		min-height: 100dvh;
		display: grid;
		place-items: center;
		padding: var(--sp-6);
		padding-top: max(var(--sp-6), var(--safe-top));
	}
</style>
