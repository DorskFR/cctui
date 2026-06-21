<script lang="ts">
	// Per-chat "soft limit reached → continue on another account" prompt (CCT-444).
	//
	// Shown when the gateway refused this session's request because cctui's own
	// share of the bound account's usage window is at cap (a 429 that stalled the
	// conversation). Offers the owner's other *same-provider* accounts; picking
	// one calls `onswitch`, which rebinds the session server-side — the worker
	// keeps running and its next upstream call lands on the chosen account.
	import { Button, Text } from '@dorsk/tsumikit';
	import type { SoftLimit } from '$lib/ws.svelte';
	import type { OAuthAccount } from '$lib/queries';
	import UsageBars from '$lib/components/molecules/UsageBars.svelte';

	let {
		softLimit,
		accounts,
		onswitch
	}: {
		softLimit: SoftLimit;
		/** The owner's accounts (for the same-provider picker). */
		accounts: OAuthAccount[];
		/** Rebind the session to `account` (name or id). Rejects async on a
		 *  provider mismatch / other failure, which we surface inline. */
		onswitch: (account: string) => Promise<void>;
	} = $props();

	// Provider *family* — both native (`anthropic`/`openai`) and `-compatible`
	// endpoints collapse to one family; cross-family switching is unsupported,
	// mirroring the server's `Family::from_provider` (CCT-444 / CCT-399).
	const family = (provider: string): 'openai' | 'anthropic' =>
		provider.includes('openai') ? 'openai' : 'anthropic';

	// The account this session is bound to (the blocked one).
	const current = $derived(accounts.find((a) => a.id === softLimit.account_id));
	// Same-family targets, excluding the blocked account itself.
	const targets = $derived(
		current
			? accounts.filter(
					(a) => a.id !== current.id && family(a.provider) === family(current.provider)
				)
			: []
	);

	// The switch in flight (account id) + any error from the last attempt.
	let switching = $state<string | null>(null);
	let error = $state<string | null>(null);

	async function pick(a: OAuthAccount) {
		if (switching) return;
		switching = a.id;
		error = null;
		try {
			await onswitch(a.id);
		} catch (e) {
			error = e instanceof Error ? e.message : 'switch failed';
			switching = null;
		}
	}
</script>

<div class="soft-limit" role="status" aria-live="polite">
	<div class="head">
		<span class="icon" aria-hidden="true">⏳</span>
		<Text size="sm" weight="medium">
			Soft limit reached on <strong>{softLimit.account_name}</strong>.
		</Text>
	</div>
	{#if targets.length}
		<Text size="xs" tone="muted">Continue this chat on another account:</Text>
		<div class="picker">
			{#each targets as a (a.id)}
				<div class="target">
					<Button
						size="sm"
						variant="default"
						disabled={switching !== null}
						loading={switching === a.id}
						onclick={() => pick(a)}
					>
						Continue on {a.name}
					</Button>
					<UsageBars
						id={a.id}
						provider={a.provider}
						cap5h={a.soft_limit_5h_pct}
						cap7d={a.soft_limit_7d_pct}
					/>
				</div>
			{/each}
		</div>
	{:else}
		<Text size="xs" tone="muted">
			No other same-provider account is available to switch to.
		</Text>
	{/if}
	{#if error}
		<Text size="xs" tone="danger">{error}</Text>
	{/if}
</div>

<style>
	.soft-limit {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
		background: var(--attention-bg);
		border-bottom: 1px solid var(--attention-bar);
	}
	.head {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		color: var(--warn);
	}
	.picker {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-2);
	}
	.target {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		min-width: 12rem;
	}
</style>
