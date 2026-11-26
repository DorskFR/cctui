<script lang="ts">
	// Per-family bindings editor: a session carries one gateway credential per
	// provider family (Claude harness + Codex spawns), and each binding is
	// switchable independently to another account holding a credential in that
	// same family. Opened from the drawer key glyph, or auto-opened on a
	// soft-limit block (the limited binding is highlighted and preselected).
	// Switching is a pure server-side rebind: the worker keeps running.
	import { Button, Field, Modal, Select, Spinner, Text } from '@dorsk/tsumikit';
	import type { SoftLimitBlock } from '#lib/ws.svelte.ts';
	import { useSessionBindings, type OAuthAccount } from '#lib/queries/index.ts';
	import { KEEP, pendingSwitches, switchRows, type SwitchRow } from './accountSwitch.logic.ts';
	import { providerLabel } from '#lib/providers.ts';
	import UsageBars from '#lib/components/molecules/UsageBars.svelte';
	import { m } from '#lib/paraglide/messages.js';

	let {
		sessionId,
		accounts,
		softLimit,
		onswitch,
		onclose
	}: {
		sessionId: string;
		/** The viewer's accounts, for the current credential's usage. */
		accounts: OAuthAccount[];
		/** Set when a soft limit triggered the open — names the limited credential. */
		softLimit: SoftLimitBlock | null;
		/** Rebind one binding to `account` (a credential id). Rejects async on
		 *  failure, which we surface inline. */
		onswitch: (account: string) => Promise<void>;
		onclose: () => void;
	} = $props();

	const bindings = useSessionBindings(() => sessionId);

	const rows: SwitchRow[] = $derived(
		switchRows(bindings.data ?? [], accounts, softLimit?.account_id ?? null)
	);

	let chosen = $state<Record<string, string>>({});

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

	const pending = $derived(pendingSwitches(rows, chosen));

	let switching = $state(false);
	let error = $state<string | null>(null);

	async function confirm() {
		if (switching || !pending.length) return;
		switching = true;
		error = null;
		for (const p of pending) {
			try {
				await onswitch(p.credId);
			} catch (e) {
				error = m.conversation_acct_switch_failed_named({
					family: providerLabel(p.family),
					account: p.name,
					error: e instanceof Error ? e.message : m.conversation_acct_switch_failed()
				});
				switching = false;
				return;
			}
		}
		onclose();
	}

	const shownCred = (r: SwitchRow) => {
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

<Modal
	title={softLimit
		? m.conversation_acct_soft_limit({ account: softLimit.account_name })
		: m.conversation_acct_switch_title()}
	tone={softLimit ? 'warn' : 'neutral'}
	busy={switching}
	{onclose}
>
	{#snippet body()}
		<div class="acct-body" data-usage-group>
			<Text as="p" tone="muted" size="sm">
				{softLimit ? m.conversation_acct_soft_desc() : m.conversation_acct_bindings_desc()}
			</Text>
			{#if bindings.isLoading}
				<Spinner label={m.common_loading()} />
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
										<option value={o.credId}>
											{o.takeover ? m.conversation_acct_takeover_option({ account: o.name }) : o.name}
										</option>
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
		</div>
	{/snippet}
	{#snippet footer()}
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
	{/snippet}
</Modal>

<style>
	.acct-body {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.binding {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-3);
		border: 1px solid var(--border);
		border-radius: var(--r-md);
	}
	.binding.limited {
		border-color: var(--danger);
	}
	.binding-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
	}
</style>
