<script lang="ts">
	import type { AccountPoolView } from '@bindings/AccountPoolView';
	import type { OAuthAccount } from '$lib/queries';
	import { useAllAccountsUsage } from '$lib/queries';
	import AccountAvatar from '$lib/components/molecules/AccountAvatar.svelte';
	import { exhaustedWindow } from '../pools.logic';
	import type { accountHandleDrag } from '../handleDrag.svelte';
	import { providerLabel } from '$lib/providers';
	import { componentNames, indicatorTone } from '$lib/components/molecules/provider-status.logic';
	import { Badge, Icon, Menu, Text, Timestamp, type MenuItem } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { fullTime, type RedirectChip } from './redirect.logic';

	let {
		account: a,
		pool,
		pools,
		drag,
		enabled,
		managed,
		canAddProvider,
		showOwner,
		compact,
		redirects,
		canRedirect,
		onedit,
		onremove,
		onmovepool,
		onaddprovider,
		onclearredirect,
		ontoggleredirect
	}: {
		account: OAuthAccount;
		pool: AccountPoolView | null;
		pools: AccountPoolView[];
		drag: ReturnType<typeof accountHandleDrag>;
		enabled: boolean;
		managed: boolean;
		canAddProvider: boolean;
		showOwner: boolean;
		compact: boolean;
		redirects: RedirectChip[];
		canRedirect: boolean;
		onedit?: () => void;
		onremove?: () => void;
		onmovepool?: (pool: AccountPoolView | null) => void;
		onaddprovider?: () => void;
		onclearredirect?: (ruleId: string) => void;
		ontoggleredirect: () => void;
	} = $props();

	const usage = useAllAccountsUsage(() => enabled);
	const exhausted = $derived(exhaustedWindow(usage.data, a.id));
	const exhaustedLabel = $derived(
		exhausted
			? m.accounts_exhausted({
					window: `${providerLabel(exhausted.provider)} ${exhausted.window.label}`
				})
			: null
	);
	// Read off the usage rows the card already fetches: the server attaches the
	// family's incident reading to a credential only while it is degraded.
	const upstream = $derived(
		(usage.data ?? [])
			.filter((e) => e.account === a.id && e.provider_status)
			.map((e) => ({ provider: e.provider, status: e.provider_status! }))
	);

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
		...(canAddProvider
			? [{ label: m.accounts_add_provider(), icon: 'plus' as const, onselect: () => onaddprovider?.() }]
			: []),
		...(canRedirect
			? [{ label: m.accounts_redirect_button(), icon: 'arrow-right' as const, onselect: ontoggleredirect }]
			: []),
		...pools
			.filter((p) => p.id !== pool?.id)
			.map((p) => ({ label: m.pools_move_to({ name: p.name }), icon: 'life-buoy' as const, onselect: () => onmovepool?.(p) })),
		...(pool ? [{ label: m.pools_leave(), onselect: () => onmovepool?.(null) }] : []),
		{ label: m.common_delete(), icon: 'trash', danger: true, onselect: () => onremove?.() }
	]);
</script>

<header class="head" class:compact>
	{#if onmovepool && !managed && !compact}
		<span
			class="handle"
			data-journey="drag-handle"
			draggable="true"
			role="img"
			aria-label={m.pools_drag_handle()}
			title={m.pools_drag_handle()}
			ondragstart={drag.dragStart}
			ondragend={drag.dragEnd}
			onpointerdown={drag.pointerDown}
			onpointermove={drag.pointerMove}
			onpointerup={drag.pointerUp}
			onpointercancel={drag.pointerCancel}>⋮⋮</span
		>
	{/if}
	<AccountAvatar emoji={a.emoji} name={a.name} id={a.id} size={28} decorative />
	<div class="title">
		<h2 class="name"><Text as="span" size="md" weight="semibold" wrap="anywhere">{a.name}</Text></h2>
		{#if meta}
			<Text as="span" tone="faint" size="xs">{meta}</Text>
		{/if}
	</div>
	{#if exhausted && exhaustedLabel}
		<span class="exhausted">
			<span class="dot"></span>
			<Text as="span" size="xs" tone="danger">
				{exhaustedLabel}{#if exhausted.window.resets_at}{' · '}<span
						title={fullTime(exhausted.window.resets_at)}
						><Timestamp
							value={exhausted.window.resets_at}
							mode="relative"
							details={false}
							tone="inherit"
						/></span
					>{/if}
			</Text>
		</span>
	{/if}
	{#each upstream as u (u.provider)}
		{@const detail = m.provider_status_account_badge({
			provider: providerLabel(u.provider),
			detail: componentNames(u.status).join(', ') || u.status.description || u.status.indicator
		})}
		<a
			class="upstream"
			class:dense={compact}
			href={u.status.url}
			target="_blank"
			rel="noopener"
			data-tone={indicatorTone(u.status.indicator)}
			title={compact ? detail : u.status.description}
			aria-label={detail}
		>
			<span class="dot"></span>
			<Text as="span" size="xs" tone="inherit">{compact ? providerLabel(u.provider) : detail}</Text>
		</a>
	{/each}
	{#each redirects as r (r.id)}
		<Badge
			size="sm"
			removable={!!onclearredirect}
			actionLabel={m.accounts_redirect_clear()}
			onremove={() => onclearredirect?.(r.id)}
			style="border-radius:var(--r-sm);flex-wrap:wrap;white-space:normal;gap:var(--sp-2)"
		>
			<Text as="span" tone="faint" size="xs">{r.family}</Text>
			<Text as="span" size="sm">{m.accounts_redirect_to({ target: r.targetName })}</Text>
			{#if r.until}
				<Text as="span" tone="faint" size="xs"
					>{m.accounts_until()}
					<span title={fullTime(r.until)}
						><Timestamp value={r.until} mode="relative" details={false} tone="inherit" /></span
					></Text
				>
			{/if}
		</Badge>
	{/each}
	<span class="spacer"></span>
	{#if compact}
		<!-- gauges only -->
	{:else if managed}
		<Text as="span" tone="faint" size="xs">{m.accounts_managed_readonly()}</Text>
	{:else}
		<span class="actions" data-journey="account-menu">
			<Menu label={m.accounts_more()} items={menu} placement="bottom-end" box="sm">
				{#snippet trigger()}<Icon name="more" size={16} />{/snippet}
			</Menu>
		</span>
	{/if}
</header>

<style>
	/* A band one shade off the card body instead of a rule under it. */
	.head {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2) var(--sp-3);
		padding: var(--sp-3) var(--sp-4);
		background: var(--bg-elevated-2);
		min-width: 0;
	}
	.head.compact {
		gap: var(--sp-1) var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
	}
	.handle {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		min-width: var(--touch-target, 2.75rem);
		min-height: var(--touch-target, 2.75rem);
		margin: calc(-1 * var(--sp-2)) 0 calc(-1 * var(--sp-2)) calc(-1 * var(--sp-2));
		color: var(--text-faint);
		letter-spacing: -0.15em;
		cursor: grab;
		user-select: none;
		/* The browser would claim a pan and cancel the pointer mid-drag. */
		touch-action: none;
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
	}
	.exhausted {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		font-variant-numeric: tabular-nums;
	}
	.dot {
		width: var(--sp-2);
		height: var(--sp-2);
		border-radius: var(--r-pill);
		background: var(--danger);
	}
	/* Wraps within the header row rather than widening it, like the exhausted
	   chip beside it. */
	.upstream {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		flex-wrap: wrap;
		text-decoration: none;
		color: var(--danger);
	}
	.upstream[data-tone='warn'] {
		color: var(--warn);
	}
	.upstream[data-tone='warn'] .dot {
		background: var(--warn);
	}
	.upstream.dense {
		flex-wrap: nowrap;
		gap: var(--sp-1);
		white-space: nowrap;
	}
	.upstream:hover {
		text-decoration: underline;
	}
	.spacer {
		flex: 1;
	}
	.actions {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
	}
</style>
