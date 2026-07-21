<script lang="ts">
	// Per-family bindings editor: a session carries one gateway credential per
	// provider family (Claude harness + Codex spawns), and each binding is
	// switchable independently to another account holding a credential in that
	// same family. Opened from the drawer key glyph, or auto-opened on a
	// soft-limit block (the limited binding is highlighted and preselected).
	// Switching is a pure server-side rebind: the worker keeps running.
	import { Button, Field, Select, Heading, Text } from '@dorsk/tsumikit';
	import type { SoftLimit } from '$lib/ws.svelte';
	import {
		useSessionBindings,
		type AccountProvider,
		type OAuthAccount,
		type SessionBinding
	} from '$lib/queries';
	import { providerFamily, providerLabel } from '$lib/providers';
	import UsageBars from '$lib/components/molecules/UsageBars.svelte';
	import { m } from '$lib/paraglide/messages';

	let {
		sessionId,
		accounts,
		softLimit,
		onswitch,
		onclose
	}: {
		sessionId: string;
		/** The owner's accounts (already scoped server-side to allowed ones). */
		accounts: OAuthAccount[];
		/** Set when a soft limit triggered the open — names the limited credential. */
		softLimit: SoftLimit | null;
		/** Rebind one binding to `account` (a credential id). Rejects async on
		 *  failure, which we surface inline. */
		onswitch: (account: string) => Promise<void>;
		onclose: () => void;
	} = $props();

	const bindings = useSessionBindings(() => sessionId);

	interface Row {
		binding: SessionBinding;
		limited: boolean;
		current: AccountProvider | null;
		options: { name: string; credId: string; provider: string }[];
	}
	const rows: Row[] = $derived(
		($bindings.data ?? []).map((b) => {
			const owner = accounts.find((a) => a.id === b.account_id);
			const current = owner?.providers.find((p) => p.id === b.credential_id) ?? null;
			const options = accounts.flatMap((a) => {
				const cred = a.providers.find((p) => providerFamily(p.provider) === b.family);
				return cred && cred.id !== b.credential_id
					? [{ name: a.name, credId: cred.id, provider: cred.provider }]
					: [];
			});
			return { binding: b, limited: softLimit?.account_id === b.credential_id, current, options };
		})
	);

	// Per-family selection; '' = keep the current binding.
	let chosen = $state<Record<string, string>>({});
	const KEEP = '';

	// A soft-limit open preselects the limited binding's first alternative so
	// the switch is one click.
	$effect(() => {
		for (const r of rows) {
			if (chosen[r.binding.family] === undefined) {
				chosen[r.binding.family] =
					softLimit && r.limited && r.options.length ? r.options[0].credId : KEEP;
			}
		}
	});

	const pending = $derived(
		rows.flatMap((r) => {
			const c = chosen[r.binding.family];
			return c && c !== KEEP ? [c] : [];
		})
	);

	let switching = $state(false);
	let error = $state<string | null>(null);

	async function confirm() {
		if (switching || !pending.length) return;
		switching = true;
		error = null;
		try {
			for (const credId of pending) await onswitch(credId);
			onclose();
		} catch (e) {
			error = e instanceof Error ? e.message : m.conversation_acct_switch_failed();
			switching = false;
		}
	}

	const shownCred = (r: Row) => {
		const c = chosen[r.binding.family];
		if (c && c !== KEEP) {
			const o = r.options.find((o) => o.credId === c);
			if (o) return { id: o.credId, provider: o.provider, softLimits: null };
		}
		return {
			id: r.binding.credential_id,
			provider: r.current?.provider ?? r.binding.family,
			softLimits: r.current?.soft_limits ?? null
		};
	};
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
		<Text as="p" tone="muted" size="sm">{m.conversation_acct_bindings_desc()}</Text>
	{/if}

	{#if $bindings.isLoading}
		<span class="spin"></span>
	{:else if rows.length === 0}
		<Text size="sm" tone="muted">{m.conversation_acct_no_bindings()}</Text>
	{:else}
		{#each rows as r (r.binding.family)}
			{@const shown = shownCred(r)}
			<div class="binding" class:limited={r.limited}>
				<div class="binding-head">
					<Text size="sm"><strong>{providerLabel(r.binding.family)}</strong> · {r.binding.account_name}</Text>
					{#if r.limited}
						<Text size="xs" tone="danger">{m.conversation_acct_limited()}</Text>
					{/if}
				</div>
				{#if r.options.length}
					<Field label={m.conversation_acct_field_label()}>
						<Select bind:value={chosen[r.binding.family]} disabled={switching}>
							<option value={KEEP}>{m.conversation_acct_keep({ account: r.binding.account_name })}</option>
							{#each r.options as o (o.credId)}
								<option value={o.credId}>{o.name}</option>
							{/each}
						</Select>
					</Field>
				{:else}
					<Text size="xs" tone="muted">{m.conversation_acct_none()}</Text>
				{/if}
				<UsageBars id={shown.id} provider={shown.provider} softLimits={shown.softLimits} />
			</div>
		{/each}
	{/if}
	{#if error}
		<Text size="xs" tone="danger">{error}</Text>
	{/if}
	<div class="acct-foot">
		<Button size="sm" variant="ghost" onclick={onclose}>{m.common_close()}</Button>
		{#if rows.some((r) => r.options.length)}
			<Button
				size="sm"
				variant="default"
				disabled={!pending.length || switching}
				loading={switching}
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
		gap: var(--sp-3);
		width: min(28rem, calc(100vw - 2rem));
		max-height: calc(100vh - 4rem);
		overflow-y: auto;
		padding: var(--sp-4);
		background: var(--bg-elevated);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-lg, var(--r-md));
		box-shadow: var(--shadow-lg, 0 8px 24px rgba(0, 0, 0, 0.5));
	}
	.binding {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-3);
		border: 1px solid var(--border-default);
		border-radius: var(--r-md);
	}
	.binding.limited {
		border-color: var(--danger, #c0392b);
	}
	.binding-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
	}
	.acct-foot {
		display: flex;
		justify-content: flex-end;
		gap: var(--sp-2);
		margin-top: var(--sp-1);
	}
	.spin {
		width: 1rem;
		height: 1rem;
		border: 2px solid var(--border-default);
		border-top-color: var(--text-muted);
		border-radius: 50%;
		animation: spin 0.8s linear infinite;
		align-self: center;
	}
	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
</style>
