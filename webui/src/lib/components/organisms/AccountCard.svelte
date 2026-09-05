<script lang="ts">
	import type { AccountProvider, OAuthAccount } from '$lib/queries';
	import AccountAvatar from '$lib/components/molecules/AccountAvatar.svelte';
	import ProviderPanel from '$lib/components/molecules/ProviderPanel.svelte';
	import ResourceShares from '$lib/components/molecules/ResourceShares.svelte';
	import { Button, Card, Heading, Select, Switch, Text, Timestamp } from '@dorsk/tsumikit';
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
		redirects = [],
		redirectTargets = [],
		onedit,
		onremove,
		onpooleligible,
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
		/** Live rules on this account, one badge each. */
		redirects?: { id: string; family: string; targetName: string; until: string | null }[];
		/** Accounts a new rule may point at, with their provider families. */
		redirectTargets?: { id: string; name: string; families: string[] }[];
		onedit?: () => void;
		onremove?: () => void;
		/** Owner-only: flip whether grantees may enrol this account in a pool. */
		onpooleligible?: (eligible: boolean) => void;
		onaddprovider?: () => void;
		oneditprovider?: (p: AccountProvider) => void;
		onreauthprovider?: (p: AccountProvider) => void;
		onremoveprovider?: (p: AccountProvider) => void;
		onsetredirect?: (targetId: string, untilHours: number | null, families: string[]) => void;
		onclearredirect?: (ruleId: string) => void;
	} = $props();

	const a = $derived(account);

	let redirectOpen = $state(false);
	let redirectTarget = $state('');
	let redirectHours = $state('');
	let redirectFamilies = $state<string[]>([]);

	// A family qualifies when both accounts carry it and no rule holds it yet.
	const availableFamilies = (t: { families: string[] }) => {
		const mine = new Set(a.providers.map((p) => p.family));
		const ruled = new Set(redirects.map((r) => r.family));
		return t.families.filter((f) => mine.has(f) && !ruled.has(f));
	};
	const openTargets = $derived(redirectTargets.filter((t) => availableFamilies(t).length > 0));
	const targetFamilies = $derived.by(() => {
		const t = openTargets.find((t) => t.id === redirectTarget);
		return t ? availableFamilies(t) : [];
	});
	$effect(() => {
		redirectFamilies = targetFamilies;
	});

	function toggleFamily(f: string) {
		redirectFamilies = redirectFamilies.includes(f)
			? redirectFamilies.filter((x) => x !== f)
			: [...redirectFamilies, f];
	}

	function submitRedirect() {
		if (!redirectTarget || redirectFamilies.length === 0) return;
		onsetredirect?.(
			redirectTarget,
			redirectHours === '' ? null : Number(redirectHours),
			redirectFamilies
		);
		redirectOpen = false;
		redirectTarget = '';
		redirectHours = '';
	}
</script>

<Card>
	<div class="acct">
		<header class="head">
			<AccountAvatar emoji={a.emoji} name={a.name} id={a.id} size={24} decorative />
			<Heading level={2} size="lg" style="min-width: 0; overflow-wrap: anywhere;">{a.name}</Heading>
			{#each redirects as r (r.id)}
				<span class="redirect-badge">
					<Text as="span" tone="faint" size="xs">{r.family}</Text>
					<Text as="span" size="sm">{m.accounts_redirect_to({ target: r.targetName })}</Text>
					{#if r.until}
						<Text as="span" tone="faint" size="xs">
							{m.accounts_redirect_until({ time: r.until })}
						</Text>
					{/if}
					{#if onclearredirect}
						<button
							type="button"
							class="redirect-clear"
							aria-label={m.common_delete()}
							onclick={() => onclearredirect(r.id)}>✕</button
						>
					{/if}
				</span>
			{/each}
			<div class="head-actions">
				{#if managed}
					<Text as="span" tone="faint" size="xs">{m.accounts_managed_readonly()}</Text>
				{:else}
					{#if onsetredirect && openTargets.length > 0}
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

		{#if redirectOpen}
			<div class="redirect-form">
				<Text as="span" tone="muted" size="sm">{m.accounts_redirect_pick()}</Text>
				<Select bind:value={redirectTarget} aria-label={m.accounts_redirect_pick()}>
					<option value="" disabled></option>
					{#each openTargets as t (t.id)}
						<option value={t.id}>{t.name}</option>
					{/each}
				</Select>
				{#each targetFamilies as f (f)}
					<label class="redirect-family">
						<input
							type="checkbox"
							checked={redirectFamilies.includes(f)}
							onchange={() => toggleFamily(f)}
						/>
						<Text as="span" size="sm">{f}</Text>
					</label>
				{/each}
				<Select bind:value={redirectHours} aria-label={m.a11y_redirect_expiry()}>
					<option value="">{m.accounts_redirect_no_expiry()}</option>
					{#each ['1', '5', '24'] as h (h)}
						<option value={h}>{m.accounts_redirect_hours({ n: h })}</option>
					{/each}
				</Select>
				<Button
					size="sm"
					variant="primary"
					disabled={!redirectTarget || redirectFamilies.length === 0}
					onclick={submitRedirect}
				>
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
				<!-- The owner's veto over an account they lend out. It sits with
				     sharing rather than with the credentials because that is what it
				     is about: a grantee can always launch on this account by name,
				     but with this off they cannot make it a silent overflow target
				     inside one of their own pools. -->
				<div class="pool-veto">
					<Switch
						checked={a.pool_eligible}
						label={m.accounts_pool_eligible()}
						onclick={() => onpooleligible?.(!a.pool_eligible)}
					/>
					<Text as="div" tone="faint" size="xs">{m.accounts_pool_eligible_hint()}</Text>
				</div>
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
	.pool-veto {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		margin-top: var(--sp-2);
	}

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
	.redirect-family {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
		cursor: pointer;
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
