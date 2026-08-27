<script lang="ts">
	import type { AccountProvider, OAuthAccount } from '$lib/queries';
	import ProviderPanel from '$lib/components/molecules/ProviderPanel.svelte';
	import ResourceShares from '$lib/components/molecules/ResourceShares.svelte';
	import { Button, Card, Heading, Select, Text, Timestamp } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	// One account identity as a full-width row: name + account actions on the
	// header line, then its provider credentials (and the sharing box) as
	// side-by-side boxes that fall to a single column when the row is narrow.
	let {
		account,
		enabled = true,
		managed = false,
		canAddProvider = false,
		canShare = false,
		showOwner = false,
		redirect = null,
		redirectTargets = [],
		onedit,
		onremove,
		onaddprovider,
		oneditprovider,
		onreauthprovider,
		onremoveprovider,
		onsetredirect,
		onclearredirect
	}: {
		account: OAuthAccount;
		/** Gates the lazy usage + shares fetches (the tab must be visible). */
		enabled?: boolean;
		/** Every provider is server-managed → the account is read-only. */
		managed?: boolean;
		canAddProvider?: boolean;
		canShare?: boolean;
		showOwner?: boolean;
		/** The live account-level rule to badge, already resolved for display. */
		redirect?: { targetName: string; until: string | null } | null;
		/** Accounts a new rule may point at (share a provider family). */
		redirectTargets?: { id: string; name: string }[];
		onedit?: () => void;
		onremove?: () => void;
		onaddprovider?: () => void;
		oneditprovider?: (p: AccountProvider) => void;
		onreauthprovider?: (p: AccountProvider) => void;
		onremoveprovider?: (p: AccountProvider) => void;
		onsetredirect?: (targetId: string, untilHours: number | null) => void;
		onclearredirect?: () => void;
	} = $props();

	const a = $derived(account);

	let redirectOpen = $state(false);
	let redirectTarget = $state('');
	let redirectHours = $state('');

	function submitRedirect() {
		if (!redirectTarget) return;
		onsetredirect?.(redirectTarget, redirectHours === '' ? null : Number(redirectHours));
		redirectOpen = false;
		redirectTarget = '';
		redirectHours = '';
	}
</script>

<Card>
	<div class="acct">
		<header class="head">
			<Heading level={2} size="lg" style="min-width: 0; overflow-wrap: anywhere;">{a.name}</Heading>
			{#if redirect}
				<span class="redirect-badge">
					<Text as="span" size="sm">{m.accounts_redirect_to({ target: redirect.targetName })}</Text>
					{#if redirect.until}
						<Text as="span" tone="faint" size="xs">
							{m.accounts_redirect_until({ time: redirect.until })}
						</Text>
					{/if}
					{#if onclearredirect}
						<button
							type="button"
							class="redirect-clear"
							aria-label={m.common_delete()}
							onclick={onclearredirect}>✕</button
						>
					{/if}
				</span>
			{/if}
			<div class="head-actions">
				{#if managed}
					<Text as="span" tone="faint" size="xs">{m.accounts_managed_readonly()}</Text>
				{:else}
					{#if !redirect && onsetredirect && redirectTargets.length > 0}
						<Button size="sm" onclick={() => (redirectOpen = !redirectOpen)}>
							{m.accounts_redirect_button()}
						</Button>
					{/if}
					{#if canAddProvider}
						<Button size="sm" onclick={onaddprovider}>{m.accounts_add_provider()}</Button>
					{/if}
					<Button size="sm" onclick={onedit}>{m.common_edit()}</Button>
					<Button size="sm" variant="danger" onclick={onremove}>{m.common_delete()}</Button>
				{/if}
			</div>
		</header>

		{#if redirectOpen && !redirect}
			<div class="redirect-form">
				<Text as="span" tone="muted" size="sm">{m.accounts_redirect_pick()}</Text>
				<Select bind:value={redirectTarget}>
					<option value="" disabled></option>
					{#each redirectTargets as t (t.id)}
						<option value={t.id}>{t.name}</option>
					{/each}
				</Select>
				<Select bind:value={redirectHours}>
					<option value="">{m.accounts_redirect_no_expiry()}</option>
					{#each ['1', '5', '24'] as h (h)}
						<option value={h}>{m.accounts_redirect_hours({ n: h })}</option>
					{/each}
				</Select>
				<Button size="sm" variant="primary" disabled={!redirectTarget} onclick={submitRedirect}>
					{m.accounts_redirect_set()}
				</Button>
				<Button size="sm" onclick={() => (redirectOpen = false)}>{m.common_cancel()}</Button>
			</div>
		{/if}

		<div class="boxes">
			{#each a.providers as p (p.id)}
				<ProviderPanel
					provider={p}
					usageEnabled={enabled}
					canManage={!p.managed}
					canRemove={!p.managed}
					onedit={() => oneditprovider?.(p)}
					onreauth={() => onreauthprovider?.(p)}
					onremove={() => onremoveprovider?.(p)}
				/>
			{:else}
				<Text tone="faint" size="sm">{m.accounts_no_credentials()}</Text>
			{/each}
			{#if canShare}
				<!-- Sharing management: owner-only surface to view/grant/revoke who
				     may USE this account. The list endpoint is owner-scoped, so only
				     render (and fetch) it for the owner/admin. -->
				<ResourceShares
					resourceType="account"
					id={a.id}
					noun={m.accounts_share_noun()}
					{enabled}
				/>
			{/if}
		</div>

		<dl class="stats">
			{#if showOwner}
				<div><dt>{m.accounts_stat_owner()}</dt><dd>{a.user_name ?? '—'}</dd></div>
			{/if}
			<div>
				<dt>{m.accounts_stat_created()}</dt>
				<dd><Timestamp value={a.created_at} mode="date" tone="inherit" /></dd>
			</div>
		</dl>
	</div>
</Card>

<style>
	/* The row is the query container: the boxes below reflow against the row's
	   own width, not the viewport's, so the layout survives being dropped into a
	   narrower shell (drawer, split pane) unchanged. */
	.acct {
		container: acct-row / inline-size;
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}
	.head {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2) var(--sp-3);
		min-width: 0;
	}
	.head-actions {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-1);
	}
	/* Horizontal on a wide row, vertical once a track can no longer hold its
	   floor — `min(100%, …)` is what makes the single-column fallback automatic
	   instead of a breakpoint. */
	.boxes {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(min(100%, calc(24rem * var(--fs-scale))), 1fr));
		align-items: start;
		gap: var(--sp-3);
		min-width: 0;
	}
	.redirect-badge {
		display: inline-flex;
		align-items: baseline;
		gap: var(--sp-2);
		padding: 0 var(--sp-2);
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		background: var(--bg-subtle);
	}
	.redirect-clear {
		border: 0;
		background: none;
		color: var(--text-muted);
		cursor: pointer;
		padding: 0;
		font-size: var(--fs-xs);
	}
	.redirect-clear:hover {
		color: var(--text);
	}
	.redirect-form {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
	}
	/* Account-level metadata is secondary to the boxes above it. */
	.stats {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-2) var(--sp-4);
		margin: 0;
		padding-top: var(--sp-2);
		border-top: 1px solid var(--border);
	}
	.stats div {
		display: flex;
		align-items: baseline;
		gap: var(--sp-2);
		min-width: 0;
	}
	.stats dt {
		color: var(--text-muted);
		font-size: var(--fs-xs);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}
	.stats dd {
		margin: 0;
		font-size: var(--fs-sm);
		font-weight: var(--fw-medium);
		overflow-wrap: anywhere;
	}
	@container acct-row (max-width: 30rem) {
		.head {
			align-items: flex-start;
			flex-direction: column;
		}
	}
</style>
