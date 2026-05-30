<script lang="ts">
	import { auth } from '$lib/auth.svelte';
	import { apiBase } from '$lib/config';

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
	<form class="card stack login-card" onsubmit={submit}>
		<div class="brand"><span class="logo">»_</span> cctui</div>
		<p class="muted">Enter an admin or user token to continue.</p>
		<div class="field">
			<label class="label" for="token">Token</label>
			<input
				id="token"
				class="input mono"
				type="password"
				autocomplete="current-password"
				placeholder="cctui token"
				bind:value={token}
			/>
		</div>
		{#if err}<div class="err">{err}</div>{/if}
		<button class="btn btn-primary btn-block" type="submit" disabled={busy || !token.trim()}>
			{#if busy}<span class="spin"></span>{:else}Sign in{/if}
		</button>
	</form>
</div>

<style>
	.login {
		min-height: 100dvh;
		display: grid;
		place-items: center;
		padding: var(--sp-6);
		padding-top: max(var(--sp-6), var(--safe-top));
	}
	.login-card {
		width: 100%;
		max-width: 22rem;
	}
	.brand {
		font-size: var(--fs-xl);
		font-weight: var(--fw-bold);
	}
	.logo {
		font-family: var(--font-mono);
		color: var(--accent);
	}
	.err {
		color: var(--danger);
		font-size: var(--fs-sm);
	}
</style>
