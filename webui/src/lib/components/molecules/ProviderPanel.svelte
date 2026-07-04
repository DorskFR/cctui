<script lang="ts">
	import type { AccountProvider } from '$lib/queries';
	import { compact } from '$lib/format';
	import UsageBars from '$lib/components/molecules/UsageBars.svelte';
	import AdapterIcon from '$lib/components/atoms/AdapterIcon.svelte';
	import { Button, Cluster, Text, Timestamp } from '@dorsk/tsumikit';
	import { providerLabel } from '$lib/providers';

	// One provider credential inside an account card (CCT-560): an account
	// identity owns an array of these (at most one per anthropic/openai family).
	// Renders the provider's own health/usage/stats and its row of actions;
	// account-level concerns (name, sharing, env, delete) stay on the card.
	let {
		provider,
		usageEnabled = true,
		canManage = false,
		canRemove = false,
		onedit,
		onreauth,
		onremove
	}: {
		provider: AccountProvider;
		/** Gates the lazy usage fetch (mirrors UsageBars' `enabled`). */
		usageEnabled?: boolean;
		/** Owner/admin and not a server-managed row → edit/remove/reauth shown. */
		canManage?: boolean;
		/** Removal also requires another provider to remain (or explicit intent). */
		canRemove?: boolean;
		onedit?: () => void;
		onreauth?: () => void;
		onremove?: () => void;
	} = $props();

	const p = $derived(provider);
	const native = $derived(p.provider === 'anthropic' || p.provider === 'openai');
</script>

<div class="provider-panel">
	<Cluster gap="var(--sp-2)" align="center" wrap={false} class="panel-head">
		<span class="provider-mark" title={providerLabel(p.provider)}>
			<AdapterIcon provider={p.provider} size={18} />
		</span>
		<Text as="span" size="sm" weight="semibold" class="panel-title">{providerLabel(p.provider)}</Text>
		{#if p.managed}
			<Text as="span" tone="faint" size="xs">managed</Text>
		{/if}
		<span class="spacer"></span>
		{#if canManage}
			{#if p.needs_reauth && native}
				<Button size="sm" variant="primary" onclick={onreauth}>Reauthenticate</Button>
			{/if}
			<Button size="sm" onclick={onedit}>Edit</Button>
			{#if canRemove}
				<Button size="sm" variant="danger" onclick={onremove} aria-label={`Remove ${providerLabel(p.provider)} provider`}>✕</Button>
			{/if}
		{/if}
	</Cluster>

	{#if p.needs_reauth}
		<!-- Credential rejected (CCT-512): the gateway saw the upstream provider
		     reject this credential's OAuth grant. -->
		<div class="reauth-banner" title={p.last_auth_error ?? undefined}>
			<Text as="span" size="xs">⚠ Credential rejected — reauthenticate</Text>
		</div>
	{/if}

	{#if native}
		<UsageBars
			id={p.id}
			provider={p.provider}
			enabled={usageEnabled}
			cap5h={p.soft_limit_5h_pct}
			cap7d={p.soft_limit_7d_pct}
		/>
	{/if}

	<dl class="stats">
		<div><dt>Requests</dt><dd>{compact(p.request_count)}</dd></div>
		<div>
			<dt>Last used</dt>
			<dd><Timestamp value={p.last_used_at} mode="relative" tone="inherit" /></dd>
		</div>
	</dl>
</div>

<style>
	.provider-panel {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-3);
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		background: var(--bg-elevated-2);
	}
	.provider-mark {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		flex: none;
		width: 1.75rem;
		height: 1.75rem;
		border-radius: var(--r-sm);
		background: var(--bg-elevated-3, var(--bg-elevated-2));
		border: 1px solid var(--border);
	}
	.spacer {
		flex: 1;
	}
	.provider-panel :global(.panel-title) {
		min-width: 0;
		overflow-wrap: anywhere;
	}
	/* Credential-rejected banner (CCT-512), same treatment as the old card. */
	.reauth-banner {
		padding: var(--sp-1) var(--sp-2);
		border: 1px solid var(--danger, #d9534f);
		border-radius: var(--r-sm);
		background: color-mix(in srgb, var(--danger, #d9534f) 12%, transparent);
		color: var(--danger, #d9534f);
	}
	/* Lightweight stat list — label over value, no input-like chrome (CCT-345). */
	.stats {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(7rem, 1fr));
		gap: var(--sp-2) var(--sp-3);
		margin: 0;
	}
	.stats div {
		min-width: 0;
	}
	.stats dt {
		color: var(--text-muted);
		font-size: var(--fs-xs);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}
	.stats dd {
		margin: 0.1rem 0 0;
		font-size: var(--fs-sm);
		font-weight: var(--fw-medium);
		overflow-wrap: anywhere;
	}
</style>
