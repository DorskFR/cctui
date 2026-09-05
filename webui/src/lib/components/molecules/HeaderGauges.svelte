<script lang="ts">
	import { Gauge, Text, Tooltip } from '@dorsk/tsumikit';
	import AccountAvatar from '$lib/components/molecules/AccountAvatar.svelte';
	import UsageBars from '$lib/components/molecules/UsageBars.svelte';
	import { useAllAccountsUsage } from '$lib/queries';
	import { settings } from '$lib/settings.svelte';
	import { providerLabel } from '$lib/providers';
	import { m } from '$lib/paraglide/messages';
	import {
		busiest,
		countdown,
		gaugeEntries,
		gaugeGroups,
		gaugePace,
		utilizationPct,
		type GaugeEntry
	} from '$lib/components/molecules/header-gauges.logic';

	const q = useAllAccountsUsage(() => settings.usageBatteries);
	const entries = $derived(settings.usageBatteries ? gaugeEntries(q.data) : []);
	const groups = $derived(gaugeGroups(entries));
	const lone = $derived(busiest(entries));

	let now = $state(Date.now());
	$effect(() => {
		const id = setInterval(() => (now = Date.now()), 30_000);
		return () => clearInterval(id);
	});

	const pctText = (e: GaugeEntry) => {
		const p = utilizationPct(e.window);
		return p === null ? m.usage_gauge_unknown() : `${p}%`;
	};
	const label = (e: GaugeEntry) =>
		m.usage_gauge_aria({
			account: e.accountName,
			provider: providerLabel(e.provider),
			pct: pctText(e)
		});
	function resetText(e: GaugeEntry): string | null {
		const at = e.window?.resets_at ? Date.parse(e.window.resets_at) : Number.NaN;
		return Number.isFinite(at) ? countdown(at - now) : null;
	}
</script>

{#snippet cell(e: GaugeEntry)}
	{@const pace = gaugePace(e.window, now)}
	{@const resets = resetText(e)}
	<Tooltip placement="bottom">
		{#snippet trigger()}
			<Gauge
				as="a"
				role="link"
				href="/accounts#{e.account}"
				value={utilizationPct(e.window) ?? 0}
				width="14px"
				height="20px"
				label={label(e)}
				title={label(e)}
			>
				{#snippet corner()}
					{#if pace}<span aria-hidden="true">{pace === 'flame' ? '🔥' : '🍃'}</span>{/if}
				{/snippet}
			</Gauge>
		{/snippet}
		{#snippet content()}
			<div class="panel">
				<Text size="sm" weight="semibold">{e.accountName} · {providerLabel(e.provider)}</Text>
				{#if resets}
					<Text size="xs" tone="faint">{m.usage_gauge_resets_in({ time: resets })}</Text>
				{/if}
				<UsageBars id={e.providerId} provider={e.provider} />
			</div>
		{/snippet}
	</Tooltip>
{/snippet}

{#if entries.length > 0}
	<span class="strip many">
		{#each groups as g (g.account)}
			<span class="group">
				<AccountAvatar
					emoji={g.accountEmoji}
					name={g.accountName}
					id={g.account}
					size={14}
					decorative
				/>
				{#each g.entries as e (e.providerId)}
					{@render cell(e)}
				{/each}
				<span class="name"><Text size="xs" tone="muted">{g.accountName}</Text></span>
			</span>
		{/each}
	</span>
	{#if lone}
		<span class="strip one">{@render cell(lone)}</span>
	{/if}
{/if}

<style>
	/* Lives in the px-pinned header: every length is px, never rem. */
	.strip {
		align-items: center;
		gap: 10px;
		flex: none;
		height: 24px;
	}
	.many {
		display: inline-flex;
	}
	.one {
		display: none;
	}
	.group {
		display: inline-flex;
		align-items: center;
		gap: 7px;
	}
	.name {
		white-space: nowrap;
	}
	@media (max-width: 640px) {
		.name {
			display: none;
		}
		.strip {
			gap: 8px;
		}
	}
	@media (max-width: 400px) {
		.many {
			display: none;
		}
		.one {
			display: inline-flex;
		}
	}
	.panel {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		min-width: 260px;
		max-width: 90vw;
	}
</style>
