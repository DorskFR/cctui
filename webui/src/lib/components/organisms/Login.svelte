<script lang="ts">
	import { auth } from '$lib/auth.svelte';
	import { apiBase } from '$lib/config';
	import { Button, Card, Field, Input, Text } from '@dorsk/tsumikit';

	let token = $state('');
	let err = $state('');
	let busy = $state(false);

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		if (!token.trim() || busy) return;
		busy = true;
		err = '';
		try {
			// Validate the token against `/me` (open to any authenticated caller —
			// admin, user, or machine) before persisting it. Probing an admin-only
			// route here used to reject valid user tokens despite the prompt
			// offering "admin or user token" (CCT-407).
			const res = await fetch(`${apiBase()}/me`, {
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
