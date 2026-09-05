<script lang="ts">
	import { auth } from '$lib/auth.svelte';
	import { PasskeyAborted, conditionalUiSupported } from '$lib/passkeys';
	import { Button, Card, Field, Input, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import type { PasskeyConfig } from '@bindings/PasskeyConfig';

	let token = $state('');
	let err = $state('');
	let busy = $state(false);
	let passkeyBusy = $state(false);
	let passkeys = $state<PasskeyConfig | null>(null);
	// Cancels a pending conditional-mediation request. A conditional `get()`
	// stays open until it is used or aborted, and only one may be in flight, so
	// the modal path has to close it first.
	let conditional: AbortController | null = null;

	const offerPasskey = $derived(!!passkeys?.available && passkeys.enrolled);

	// Probe the server once. When the passkey button is on offer we also arm the
	// browser's autofill flow, so the key shows up in the token field's
	// dropdown; that is silent and dismissible, unlike the modal, which we open
	// only on a click or when the server says to.
	$effect(() => {
		let cancelled = false;
		void (async () => {
			const cfg = await auth.passkeyConfig();
			if (cancelled) return;
			passkeys = cfg;
			if (!cfg?.available || !cfg.enrolled) return;
			if (cfg.auto_prompt) {
				await signInWithPasskey();
			} else if (await conditionalUiSupported()) {
				armConditional();
			}
		})();
		return () => {
			cancelled = true;
			conditional?.abort();
		};
	});

	/** Offer the passkey from the token field's autocomplete. Never interrupts:
	 *  it resolves only if the user picks the key, and a failure leaves the form
	 *  exactly as it was. On success `auth.isAuthed` flips and the layout swaps
	 *  this screen out on its own. */
	function armConditional() {
		conditional = new AbortController();
		void auth.loginWithPasskey('conditional', conditional.signal).catch(() => {});
	}

	async function signInWithPasskey() {
		if (passkeyBusy) return;
		// A conditional request already holds the authenticator; drop it or the
		// modal call is rejected outright.
		conditional?.abort();
		conditional = null;
		passkeyBusy = true;
		err = '';
		try {
			const ok = await auth.loginWithPasskey();
			if (!ok) err = m.login_passkey_failed();
		} catch (e) {
			// A dismissed dialog is a choice, not an error worth shouting about.
			if (!(e instanceof PasskeyAborted)) err = m.login_passkey_failed();
		} finally {
			passkeyBusy = false;
		}
	}

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
	<Card as="form" class="stack" maxWidth="22rem" onsubmit={submit}>
		<Text size="xl" weight="bold" class="brand"><Text variant="code" tone="accent">»_</Text> cctui</Text>
		<Text as="p" tone="muted">{m.login_subtitle()}</Text>
		<Field label={m.login_token_label()}>
			<Input
				mono
				type="password"
				autocomplete={offerPasskey ? 'current-password webauthn' : 'current-password'}
				placeholder={m.login_token_placeholder()}
				bind:value={token}
			/>
		</Field>
		{#if err}<Text as="div" tone="danger" size="sm">{err}</Text>{/if}
		<Button variant="primary" block type="submit" disabled={busy || !token.trim()}>
			{#if busy}<span class="spin"></span>{:else}{m.login_sign_in()}{/if}
		</Button>
		{#if offerPasskey}
			<Button block type="button" disabled={passkeyBusy} onclick={signInWithPasskey}>
				{#if passkeyBusy}<span class="spin"></span>{:else}{m.login_passkey_button()}{/if}
			</Button>
		{/if}
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
