<script lang="ts">
	import { useAccounts } from '$lib/queries';
	import { compact } from '$lib/format';
	import { providerLabel } from '$lib/providers';
	import UsageBars from '$lib/components/molecules/UsageBars.svelte';
	import AdapterIcon from '$lib/components/atoms/AdapterIcon.svelte';
	import AccountAvatar from '$lib/components/molecules/AccountAvatar.svelte';
	import { Text, Timestamp } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	// Every account the viewer can see, one block per provider that has a
	// usage API: the same gauges the Accounts screen shows (5h / weekly / …),
	// trimmed to name, bars and the request / last-used pair.
	const accounts = useAccounts();
	const HAS_USAGE = new Set(['anthropic', 'openai', 'fireworks']);
	const rows = $derived(
		(accounts.data ?? []).flatMap((a) =>
			a.providers
				.filter((p) => HAS_USAGE.has(p.provider))
				.map((p) => ({ account: a.name, accountId: a.id, emoji: a.emoji, provider: p }))
		)
	);
</script>

{#if accounts.isLoading}
	<Text tone="faint" size="sm">{m.common_loading()}</Text>
{:else if rows.length === 0}
	<Text tone="faint" size="sm">{m.stats_dock_no_accounts()}</Text>
{:else}
	<div class="list">
		{#each rows as r (r.provider.id)}
			<div class="acct">
				<div class="head">
					<span class="mark" title={providerLabel(r.provider.provider)}>
						<AdapterIcon provider={r.provider.provider} size={14} />
					</span>
					<AccountAvatar emoji={r.emoji} name={r.account} id={r.accountId} size={16} decorative />
					<Text as="span" size="sm" weight="semibold" truncate>{r.account}</Text>
					<Text as="span" size="xs" tone="faint">{providerLabel(r.provider.provider)}</Text>
				</div>
				<UsageBars id={r.provider.id} provider={r.provider.provider} softLimits={r.provider.soft_limits} />
				<div class="meta">
					<Text as="span" size="xs" tone="faint">{m.providers_requests({ n: compact(r.provider.request_count) })}</Text>
					<Text as="span" size="xs" tone="faint">
						{m.providers_used()} <Timestamp value={r.provider.last_used_at} mode="relative" tone="inherit" />
					</Text>
				</div>
			</div>
		{/each}
	</div>
{/if}

<style>
	.list {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	/* NOT `.row`: tsumikit ships a global `.row` utility (flex, align-items:
	   center). A scoped `.row` here only overrode the direction, so the centering
	   survived and every child was sized to its content — a bar wider than the
	   card then overflowed on BOTH sides and got clipped by the panel. */
	.acct {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-2);
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		background: var(--bg-elevated-2);
	}
	.head {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	.mark {
		display: inline-flex;
		flex: none;
	}
	.meta {
		display: flex;
		justify-content: space-between;
		gap: var(--sp-2);
		flex-wrap: wrap;
	}
</style>
