<script lang="ts">
	import { useQueryClient } from '@tanstack/svelte-query';
	import { Popover, Text } from '@dorsk/tsumikit';
	import { now as clockNow } from '$lib/clock.svelte';
	import { settings } from '$lib/settings.svelte';
	import { ws } from '$lib/ws.svelte';
	import {
		qk,
		useAccounts,
		useAllAccountsUsage,
		useMachineResources,
		useProviderStatus,
		useRedirectChips,
		usageKeys
	} from '$lib/queries';
	import type { AccountUsageEntry } from '$lib/queries';
	import type { MachineResourcesRow } from '@bindings/MachineResourcesRow';
	import AccountCard from '$lib/components/organisms/AccountCard.svelte';
	import { patchUsageEntries } from '$lib/components/molecules/usage-battery.logic';
	import { patchRow, pinnedRows } from '$lib/components/molecules/resource-gauge.logic';
	import {
		degraded,
		indicatorTone,
		worstIndicator
	} from '$lib/components/molecules/provider-status.logic';
	import MachineHealthList from './MachineHealthList.svelte';
	import ProviderIncidents from './ProviderIncidents.svelte';
	import { machineHealth, worseTone, worstUsage } from './header-status.logic';
	import { m } from '$lib/paraglide/messages';

	const qc = useQueryClient();
	const now = $derived(clockNow(30_000));

	const usage = useAllAccountsUsage(() => true);
	$effect(() =>
		ws.onAccountUsage((ev) => {
			qc.setQueryData<AccountUsageEntry[]>(usageKeys.all(), (old) =>
				patchUsageEntries(old, ev.account_id, ev.usage)
			);
			qc.setQueryData(usageKeys.one(ev.account_id), ev.usage);
		})
	);
	const accounts = useAccounts();
	const chips = useRedirectChips();
	const capsFor = (providerId: string) => {
		for (const a of accounts.data ?? []) {
			const p = a.providers.find((p) => p.id === providerId);
			if (p) return p.soft_limits;
		}
		return null;
	};
	const worst = $derived(worstUsage(usage.data, capsFor));
	const pinnedAccounts = $derived.by(() => {
		const ids = new Set((usage.data ?? []).filter((r) => r.header_pin).map((r) => r.account));
		return (accounts.data ?? [])
			.filter((a) => ids.has(a.id))
			.map((a) => ({ ...a, providers: a.providers.filter((p) => p.header_pin) }));
	});

	const pinnedMachines = $derived(settings.monitoredMachines);
	const resources = useMachineResources(() => pinnedMachines.length > 0);
	$effect(() =>
		ws.onMachineResources((ev) => {
			qc.setQueryData<MachineResourcesRow[]>(qk.machineResources, (old) =>
				patchRow(old, ev.machine_id, ev.resources, new Date().toISOString())
			);
		})
	);
	const machines = $derived(pinnedRows(resources.data, pinnedMachines));

	const providers = useProviderStatus();
	const incidents = $derived(degraded(providers.data));
	const providerTone = $derived(indicatorTone(worstIndicator(providers.data)));

	const dot = $derived(worseTone(machineHealth(machines, now), providerTone));
	const usageText = $derived(worst ? `${worst.short} ${worst.pct}%` : null);
	const visible = $derived(!!worst || machines.length > 0 || incidents.length > 0);
	const label = $derived(m.shell_status_aria({ usage: usageText ?? m.shell_status_usage_none() }));
</script>

{#if visible}
	<Popover {label} placement="bottom-end" bare>
		{#snippet trigger()}
			<span
				class="status"
				data-tone={worst?.tone ?? 'ok'}
				title={worst
					? m.shell_status_usage_title({
							window: worst.short,
							pct: worst.pct,
							account: worst.accountName
						})
					: undefined}
			>
				{#if usageText}<span class="usage">{usageText}</span>{/if}
				{#if dot}<span class="dot" data-tone={dot} aria-hidden="true"></span>{/if}
			</span>
		{/snippet}
		<div class="panel">
			{#if incidents.length > 0}
				<section>
					<Text variant="eyebrow" tone="muted">{m.shell_status_providers_heading()}</Text>
					<ProviderIncidents {incidents} />
				</section>
			{/if}
			{#if pinnedAccounts.length > 0}
				<section>
					<Text variant="eyebrow" tone="muted">{m.shell_status_usage_heading()}</Text>
					{#each pinnedAccounts as acct (acct.id)}
						<AccountCard account={acct} compact redirects={chips.chipsFor(acct.id)} />
					{/each}
					<a class="more" href="/accounts">{m.shell_status_open_accounts()}</a>
				</section>
			{/if}
			{#if machines.length > 0}
				<section>
					<Text variant="eyebrow" tone="muted">{m.shell_status_machines_heading()}</Text>
					<MachineHealthList rows={machines} {now} />
				</section>
			{/if}
		</div>
	</Popover>
{/if}

<style>
	/* Lives in the px-pinned header: every length is px, never rem. */
	.status {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: 6px;
		min-width: max(32px, var(--hit-min, 24px));
		min-height: max(32px, var(--hit-min, 24px));
		padding: 0 10px;
		border-radius: var(--r-pill);
		color: var(--text-muted);
		font-family: var(--font-numeric, inherit);
		font-variant-numeric: tabular-nums;
		font-size: 13px;
		font-weight: 600;
		white-space: nowrap;
	}
	.status:hover {
		background: var(--bg-elevated-2);
		color: var(--text);
	}
	.status[data-tone='warn'] .usage {
		color: var(--warn);
	}
	.status[data-tone='danger'] .usage {
		color: var(--danger);
	}
	.dot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		flex: none;
		background: var(--ok);
	}
	.dot[data-tone='warn'] {
		background: var(--warn);
	}
	.dot[data-tone='danger'] {
		background: var(--danger);
	}
	.dot[data-tone='unknown'] {
		background: var(--dot-dead);
	}
	.panel {
		display: flex;
		flex-direction: column;
		gap: var(--sp-4);
		/* The kit caps .pop-panel at 22rem and spends sp-1 + 1px border on chrome. */
		width: 21rem;
		max-width: 100%;
		max-height: 75dvh;
		overflow: hidden auto;
		overscroll-behavior: contain;
	}
	section {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.more {
		align-self: flex-start;
		color: var(--accent);
		font-size: var(--fs-sm);
		text-decoration: none;
	}
	.more:hover {
		text-decoration: underline;
	}
</style>
