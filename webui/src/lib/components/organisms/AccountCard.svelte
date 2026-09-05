<script lang="ts">
	import type { AccountPoolView } from '@bindings/AccountPoolView';
	import type { AccountProvider, OAuthAccount } from '$lib/queries';
	import { useAllAccountsUsage, useResourceShares } from '$lib/queries';
	import AccountAvatar from '$lib/components/molecules/AccountAvatar.svelte';
	import ResourceShares from '$lib/components/molecules/ResourceShares.svelte';
	import ProviderColumn from '$lib/components/organisms/accounts/ProviderColumn.svelte';
	import { ACCOUNT_DRAG_MIME, exhaustedWindow } from '$lib/components/organisms/accounts/pools.logic';
	import { accountDrag } from '$lib/components/organisms/accounts/drag.svelte';
	import { resetIn } from '$lib/components/molecules/cap-bar.logic';
	import { providerLabel } from '$lib/providers';
	import { Button, IconButton, Menu, Select, Text, Timestamp, type MenuItem } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		account,
		pool = null,
		pools = [],
		enabled = true,
		managed = false,
		canAddProvider = false,
		canShare = false,
		showOwner = false,
		redirects = [],
		redirectTargets = [],
		onedit,
		onremove,
		onmovepool,
		onaddprovider,
		oneditprovider,
		onreauthprovider,
		onremoveprovider,
		onsetredirect,
		onclearredirect
	}: {
		account: OAuthAccount;
		/** The pool this account sits in, if any. */
		pool?: AccountPoolView | null;
		/** Every pool the account may be moved to; the menu is the no-drag path. */
		pools?: AccountPoolView[];
		/** Gates the lazy usage + shares fetches (the tab must be visible). */
		enabled?: boolean;
		/** Every provider is server-managed → the account is read-only. */
		managed?: boolean;
		canAddProvider?: boolean;
		canShare?: boolean;
		showOwner?: boolean;
		redirects?: { id: string; family: string; targetName: string; until: string | null }[];
		redirectTargets?: { id: string; name: string; families: string[] }[];
		onedit?: () => void;
		onremove?: () => void;
		onmovepool?: (pool: AccountPoolView | null) => void;
		onaddprovider?: () => void;
		oneditprovider?: (p: AccountProvider) => void;
		onreauthprovider?: (p: AccountProvider) => void;
		onremoveprovider?: (p: AccountProvider) => void;
		onsetredirect?: (targetId: string, untilHours: number | null, families: string[]) => void;
		onclearredirect?: (ruleId: string) => void;
	} = $props();

	const a = $derived(account);

	const usage = useAllAccountsUsage(() => enabled);
	const exhausted = $derived(exhaustedWindow(usage.data, a.id));
	const exhaustedText = $derived.by(() => {
		if (!exhausted) return null;
		const time = resetIn(exhausted.window.resets_at, Date.now());
		const label = `${providerLabel(exhausted.provider)} ${exhausted.window.label}`;
		return time
			? m.accounts_exhausted_resets({ window: label, time })
			: m.accounts_exhausted({ window: label });
	});

	const shares = useResourceShares(
		() => 'account',
		() => a.id,
		() => canShare && enabled
	);
	const sharedWith = $derived((shares.data ?? []).map((s) => s.user_name).join(', '));
	let sharingOpen = $state(false);

	const meta = $derived(
		[
			showOwner && a.user_name ? m.accounts_owner_meta({ owner: a.user_name }) : null,
			pool ? m.accounts_pool_meta({ pool: pool.name }) : null
		]
			.filter(Boolean)
			.join(' · ')
	);

	const menu = $derived<MenuItem[]>([
		{ label: m.common_edit(), icon: 'edit', onselect: () => onedit?.() },
		...pools
			.filter((p) => p.id !== pool?.id)
			.map((p) => ({ label: m.pools_move_to({ name: p.name }), onselect: () => onmovepool?.(p) })),
		...(pool ? [{ label: m.pools_leave(), onselect: () => onmovepool?.(null) }] : []),
		{ label: m.common_delete(), icon: 'trash', danger: true, onselect: () => onremove?.() }
	]);

	function dragStart(e: DragEvent) {
		if (!e.dataTransfer) return;
		e.dataTransfer.setData(ACCOUNT_DRAG_MIME, a.id);
		e.dataTransfer.effectAllowed = 'move';
		accountDrag.accountId = a.id;
	}
	function dragEnd() {
		accountDrag.accountId = '';
	}

	let redirectOpen = $state(false);
	let redirectTarget = $state('');
	let redirectHours = $state('');

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
	let redirectFamilies = $derived<string[]>(targetFamilies);

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

<article class="acct" id={a.id}>
	<header class="head">
		{#if onmovepool && !managed}
			<span
				class="handle"
				draggable="true"
				role="img"
				aria-label={m.pools_drag_handle()}
				title={m.pools_drag_handle()}
				ondragstart={dragStart}
				ondragend={dragEnd}>⋮⋮</span
			>
		{/if}
		<AccountAvatar emoji={a.emoji} name={a.name} id={a.id} size={28} decorative />
		<div class="title">
			<h2 class="name"><Text as="span" size="md" weight="semibold">{a.name}</Text></h2>
			{#if meta}
				<Text as="span" tone="faint" size="xs">{meta}</Text>
			{/if}
		</div>
		{#if exhaustedText}
			<span class="exhausted"><span class="dot"></span><Text as="span" size="xs" tone="danger">{exhaustedText}</Text></span>
		{/if}
		{#each redirects as r (r.id)}
			<span class="redirect-badge">
				<Text as="span" tone="faint" size="xs">{r.family}</Text>
				<Text as="span" size="sm">{m.accounts_redirect_to({ target: r.targetName })}</Text>
				{#if r.until}
					<Text as="span" tone="faint" size="xs">{m.accounts_redirect_until({ time: r.until })}</Text>
				{/if}
				{#if onclearredirect}
					<IconButton icon="x" label={m.common_delete()} inline size={12} onclick={() => onclearredirect(r.id)} />
				{/if}
			</span>
		{/each}
		<span class="spacer"></span>
		{#if managed}
			<Text as="span" tone="faint" size="xs">{m.accounts_managed_readonly()}</Text>
		{:else}
			{#if onsetredirect && openTargets.length > 0}
				<Button size="sm" variant="ghost" onclick={() => (redirectOpen = !redirectOpen)}>
					{m.accounts_redirect_button()}
				</Button>
			{/if}
			{#if canAddProvider}
				<Button size="sm" variant="ghost" onclick={onaddprovider}>{m.accounts_add_provider()}</Button>
			{/if}
			<Menu label={m.accounts_more()} items={menu} bare placement="bottom-end">
				{#snippet trigger()}
					<IconButton icon="more" label={m.accounts_more()} variant="ghost" box="sm" />
				{/snippet}
			</Menu>
		{/if}
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
					<input type="checkbox" checked={redirectFamilies.includes(f)} onchange={() => toggleFamily(f)} />
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

	<div class="columns">
		{#each a.providers as p (p.id)}
			<ProviderColumn
				provider={p}
				usageEnabled={enabled}
				canManage={!p.managed}
				canRemove={!p.managed}
				onedit={() => oneditprovider?.(p)}
				onreauth={() => onreauthprovider?.(p)}
				onremove={() => onremoveprovider?.(p)}
			/>
		{:else}
			<div class="none"><Text tone="faint" size="sm">{m.accounts_no_credentials()}</Text></div>
		{/each}
	</div>

	<footer class="foot">
		{#if canShare}
			<Text as="span" size="xs" tone="muted">
				{#if sharedWith}
					{m.accounts_shared_with()} <Text as="span" size="xs" weight="semibold" tone="default">{sharedWith}</Text>
				{:else}
					{m.accounts_shared_none()}
				{/if}
			</Text>
			<Button variant="link" size="sm" onclick={() => (sharingOpen = !sharingOpen)}>
				{m.accounts_manage_sharing()}
			</Button>
		{/if}
		<span class="spacer"></span>
		<Text as="span" size="xs" tone="faint">
			{m.accounts_created()} <Timestamp value={a.created_at} mode="short-iso" mono tone="inherit" />
		</Text>
	</footer>
	{#if canShare && sharingOpen}
		<div class="sharing">
			<ResourceShares resourceType="account" id={a.id} noun={m.accounts_share_noun()} {enabled} />
		</div>
	{/if}
</article>

<style>
	.acct {
		container: acct-row / inline-size;
		display: flex;
		flex-direction: column;
		min-width: 0;
		border: 1px solid var(--border);
		border-radius: var(--r-lg);
		background: var(--bg-elevated);
		overflow: hidden;
	}
	.head {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2) var(--sp-3);
		padding: var(--sp-3) var(--sp-4);
		border-bottom: 1px solid var(--border);
		min-width: 0;
	}
	.handle {
		color: var(--text-faint);
		letter-spacing: -0.15em;
		cursor: grab;
		user-select: none;
	}
	.handle:active {
		cursor: grabbing;
	}
	.title {
		display: flex;
		align-items: baseline;
		gap: var(--sp-2);
		min-width: 0;
	}
	.name {
		margin: 0;
		min-width: 0;
		overflow-wrap: anywhere;
	}
	.exhausted {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.dot {
		width: var(--sp-2);
		height: var(--sp-2);
		border-radius: var(--r-pill);
		background: var(--danger);
	}
	.spacer {
		flex: 1;
	}
	.redirect-badge {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		padding: 0 var(--sp-2);
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		background: var(--bg);
	}
	.redirect-form {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-4);
		border-bottom: 1px solid var(--border);
	}
	.redirect-family {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
		cursor: pointer;
	}
	.columns {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
	}
	.none {
		padding: var(--sp-3) var(--sp-4);
	}
	.foot {
		display: flex;
		align-items: center;
		gap: var(--sp-4);
		padding: var(--sp-2) var(--sp-4);
		background: var(--bg);
		border-top: 1px solid var(--border);
	}
	.sharing {
		padding: var(--sp-3) var(--sp-4);
		border-top: 1px solid var(--border);
	}
	@container acct-row (max-width: 66rem) {
		.columns {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}
	}
	@container acct-row (max-width: 34rem) {
		.columns {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
