<script lang="ts">
	// At-will "switch this chat to another account" modal (CCT-444 follow-up).
	//
	// Opened from the key glyph in the drawer header, or auto-opened when the
	// gateway refuses a request because cctui's share of the bound account's usage
	// window hit its soft cap. Lists the owner's *other same-provider* accounts
	// (the only ones the server will accept — the worker's harness already
	// negotiated the auth scheme, so a cross-family rebind is rejected with 409).
	// Picking one calls `onswitch`, a pure server-side rebind: the worker keeps
	// running and its next upstream call lands on the chosen account.
	import { Button, Field, Heading, Select, Text } from '@dorsk/tsumikit';
	import type { SoftLimit } from '$lib/ws.svelte';
	import { primaryProvider, type OAuthAccount } from '$lib/queries';
	import UsageBars from '$lib/components/molecules/UsageBars.svelte';
	import { m } from '$lib/paraglide/messages';

	let {
		currentName,
		accounts,
		softLimit,
		onswitch,
		onclose
	}: {
		/** Name of the account this session is currently bound to. */
		currentName: string | null;
		/** The owner's accounts (already scoped server-side to allowed ones). */
		accounts: OAuthAccount[];
		/** Set when a soft limit triggered the open — drives the heading/notice. */
		softLimit: SoftLimit | null;
		/** Rebind the session to `account` (name or id). Rejects async on a
		 *  provider mismatch / other failure, which we surface inline. */
		onswitch: (account: string) => Promise<void>;
		onclose: () => void;
	} = $props();

	// Provider *family* — both native (`anthropic`/`openai`) and `-compatible`
	// endpoints collapse to one family; cross-family switching is unsupported,
	// mirroring the server's `Family::from_provider` (CCT-444 / CCT-399).
	const family = (provider: string): 'openai' | 'anthropic' =>
		provider.includes('openai') ? 'openai' : 'anthropic';
	// TODO(CCT-560): single-provider back-compat — reads providers[0].
	const providerOf = (a: OAuthAccount) => primaryProvider(a)?.provider ?? '';

	// The account this session is bound to. A soft-limit open carries the id —
	// the *credential* (provider-row) id, so match against providers (CCT-565;
	// only backfilled accounts share the identity id by uuid reuse). An at-will
	// open only has the name (SessionListItem exposes no account id).
	const current = $derived(
		softLimit
			? accounts.find(
					(a) => a.providers.some((p) => p.id === softLimit.account_id) || a.id === softLimit.account_id
				)
			: accounts.find((a) => a.name === currentName)
	);
	// Same-family targets, excluding the current account itself.
	const targets = $derived(
		current
			? accounts.filter(
					(a) => a.id !== current.id && family(providerOf(a)) === family(providerOf(current))
				)
			: []
	);

	// The same-family CREDENTIAL id for a target, not the identity id (CCT-565):
	// the switch endpoint rebinds session_tokens.account_id, which points at
	// provider rows. (The server also resolves identity ids as a fallback, but
	// only backfilled accounts share the two by uuid reuse.)
	const credIdOf = (a: OAuthAccount) =>
		a.providers.find((p) => current && family(p.provider) === family(providerOf(current)))?.id ??
		a.id;

	// Options keyed by the credential id that gets sent to the switch endpoint.
	const options = $derived(targets.map((a) => ({ account: a, credId: credIdOf(a) })));

	// The selected credential id (nothing chosen until the user picks one).
	let chosen = $state<string | null>(null);
	const selected = $derived(options.find((o) => o.credId === chosen)?.account ?? null);

	// A soft-limit open preselects the first target so the switch is one click.
	$effect(() => {
		if (softLimit && chosen === null && options.length) chosen = options[0].credId;
	});

	// The switch in flight (credential id) + any error from the last attempt.
	let switching = $state<string | null>(null);
	let error = $state<string | null>(null);

	async function confirm() {
		if (switching || !chosen) return;
		switching = chosen;
		error = null;
		try {
			await onswitch(chosen);
			onclose();
		} catch (e) {
			error = e instanceof Error ? e.message : m.conversation_acct_switch_failed();
			switching = null;
		}
	}
</script>

<div
	class="acct-scrim"
	role="button"
	tabindex="-1"
	aria-label={m.conversation_acct_close_aria()}
	onclick={onclose}
	onkeydown={(e) => e.key === 'Escape' && onclose()}
></div>
<div class="acct-modal" role="dialog" aria-modal="true" aria-label={m.conversation_acct_switch_aria()}>
	{#if softLimit}
		<Heading level={3}>{m.conversation_acct_soft_limit({ account: softLimit.account_name })}</Heading>
		<Text as="p" tone="muted" size="sm">
			{m.conversation_acct_soft_desc()}
		</Text>
	{:else}
		<Heading level={3}>{m.conversation_acct_switch_title()}</Heading>
		<Text as="p" tone="muted" size="sm">
			{#if current}
				{m.conversation_acct_running_pre()}<strong>{current.name}</strong>{m.conversation_acct_running_post()}
			{:else}
				{m.conversation_acct_pick()}
			{/if}
		</Text>
	{/if}

	{#if options.length}
		<div class="picker">
			<Field label={m.conversation_acct_field_label()}>
				<Select bind:value={chosen} disabled={switching !== null}>
					<option value={null} disabled>{m.conversation_acct_select_placeholder()}</option>
					{#each options as o (o.credId)}
						<option value={o.credId}>{o.account.name}</option>
					{/each}
				</Select>
			</Field>
			{#if selected}
				<UsageBars
					id={primaryProvider(selected)?.id ?? selected.id}
					provider={providerOf(selected)}
					softLimits={primaryProvider(selected)?.soft_limits ?? null}
				/>
			{/if}
		</div>
	{:else}
		<Text size="sm" tone="muted">
			{m.conversation_acct_none()}
		</Text>
	{/if}
	{#if error}
		<Text size="xs" tone="danger">{error}</Text>
	{/if}
	<div class="acct-foot">
		<Button size="sm" variant="ghost" onclick={onclose}>{m.common_close()}</Button>
		{#if options.length}
			<Button
				size="sm"
				variant="default"
				disabled={!chosen || switching !== null}
				loading={switching !== null}
				onclick={confirm}
			>
				{m.conversation_acct_switch_btn()}
			</Button>
		{/if}
	</div>
</div>

<style>
	.acct-scrim {
		position: fixed;
		inset: 0;
		z-index: 70;
		background: rgba(0, 0, 0, 0.45);
		border: none;
	}
	.acct-modal {
		position: fixed;
		z-index: 71;
		top: 50%;
		left: 50%;
		transform: translate(-50%, -50%);
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		width: min(28rem, calc(100vw - 2rem));
		max-height: calc(100vh - 4rem);
		overflow-y: auto;
		padding: var(--sp-4);
		background: var(--bg-elevated);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-lg, var(--r-md));
		box-shadow: var(--shadow-lg, 0 8px 24px rgba(0, 0, 0, 0.5));
	}
	.picker {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.acct-foot {
		display: flex;
		justify-content: flex-end;
		gap: var(--sp-2);
		margin-top: var(--sp-1);
	}
</style>
