<script lang="ts">
	import { auth } from '$lib/auth.svelte';
	import { Button, Card, Field, Input, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let token = $state('');
	let err = $state('');
	let busy = $state(false);

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		if (!token.trim() || busy) return;
		busy = true;
		err = '';
		try {
			// `auth.login` validates the token server-side and, on success, sets the
			// `HttpOnly` auth cookie. A bad token resolves to 401 → false.
			// Any authenticated principal (admin, user, machine) is accepted.
			const ok = await auth.login(token.trim());
			if (!ok) err = m.login_invalid_token();
		} catch {
			err = m.login_server_unreachable();
		} finally {
			busy = false;
		}
	}
</script>

<div class="login">
	<Card as="form" class="stack" style="width:100%;max-width:22rem" onsubmit={submit}>
		<Text size="xl" weight="bold" class="brand"><Text variant="code" tone="accent">»_</Text> cctui</Text>
		<Text as="p" tone="muted">{m.login_subtitle()}</Text>
		<Field label={m.login_token_label()} for="token">
			<Input
				id="token"
				mono
				type="password"
				autocomplete="current-password"
				placeholder={m.login_token_placeholder()}
				bind:value={token}
			/>
		</Field>
		{#if err}<Text as="div" tone="danger" size="sm">{err}</Text>{/if}
		<Button variant="primary" block type="submit" disabled={busy || !token.trim()}>
			{#if busy}<span class="spin"></span>{:else}{m.login_sign_in()}{/if}
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
