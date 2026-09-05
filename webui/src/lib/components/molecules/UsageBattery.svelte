<script lang="ts">
	import { Popover, Text } from '@dorsk/tsumikit';
	import AdapterIcon from '$lib/components/atoms/AdapterIcon.svelte';
	import AccountCard from '$lib/components/organisms/AccountCard.svelte';
	import { useAccounts, useAllAccountsUsage } from '$lib/queries';
	import type { UsageWindow } from '$lib/queries';
	import { providerLabel } from '$lib/providers';
	import { m } from '$lib/paraglide/messages';
	import {
		aggregateBars,
		barPct,
		batteryEntries,
		countdown,
		headroomTone,
		paceState,
		wallInMs,
		worstPace,
		type BatteryBars,
		type BatteryEntry
	} from '$lib/components/molecules/usage-battery.logic';

	const q = useAllAccountsUsage(() => true);
	// The popovers show the same read-only AccountCard the stats dock shows.
	const accounts = useAccounts();
	const accountOf = (id: string) => (accounts.data ?? []).find((a) => a.id === id);
	const entries = $derived(batteryEntries(q.data));
	const groups = $derived.by(() => {
		const byAccount = new Map<string, BatteryEntry[]>();
		for (const e of entries) byAccount.set(e.account, [...(byAccount.get(e.account) ?? []), e]);
		return [...byAccount.values()];
	});
	const agg = $derived(aggregateBars(entries));

	let now = $state(Date.now());
	$effect(() => {
		const id = setInterval(() => (now = Date.now()), 30_000);
		return () => clearInterval(id);
	});

	const pctText = (w: UsageWindow | null) => {
		const p = barPct(w);
		return p === null ? m.usage_battery_unknown() : `${p}%`;
	};
	function barText(label: string, w: UsageWindow | null): string {
		const reset = w?.resets_at ? Date.parse(w.resets_at) - now : null;
		const tail =
			reset !== null && Number.isFinite(reset)
				? ` (${m.usage_battery_resets_in({ time: countdown(reset) })})`
				: '';
		return `${label} ${pctText(w)}${tail}`;
	}
	function paceOf(bars: BatteryBars) {
		const w = worstPace([bars.fiveHour, bars.weekly]);
		const state = paceState(w?.pace);
		if (!w || !state) return null;
		const wallMs = wallInMs(w.pace, w.resets_at, now);
		const text = [
			state === 'leaf'
				? m.usage_battery_pace_leaf()
				: state === 'flame'
					? m.usage_battery_pace_flame()
					: m.usage_battery_pace_neutral(),
			m.usage_battery_pace_expected({ expected: Math.round(w.pace?.expected_pct ?? 0) })
		];
		if (wallMs !== null) text.push(m.usage_battery_pace_wall({ time: countdown(wallMs) }));
		return {
			state,
			glyph: state === 'leaf' ? '🍃' : state === 'flame' ? '🔥' : '•',
			wall: wallMs === null ? null : countdown(wallMs),
			text: text.join(' · ')
		};
	}
	function titleOf(head: string, bars: BatteryBars): string {
		const lines = [
			head,
			barText(m.usage_battery_five_hour(), bars.fiveHour),
			barText(m.usage_battery_weekly(), bars.weekly)
		];
		const p = paceOf(bars);
		if (p) lines.push(p.text);
		return lines.join('\n');
	}
</script>

{#snippet bar(w: UsageWindow | null)}
	{@const pct = barPct(w)}
	<span class="track" data-tone={headroomTone(pct)}>
		{#if pct !== null}<span class="fill" style={`width: ${pct}%`}></span>{/if}
	</span>
{/snippet}

{#snippet cell(bars: BatteryBars, title: string, provider: string | null)}
	{@const p = paceOf(bars)}
	<span class="cell" {title}>
		{#if provider}<span class="glyph"><AdapterIcon {provider} size={12} /></span>{/if}
		<span class="bars">
			{@render bar(bars.fiveHour)}
			{@render bar(bars.weekly)}
		</span>
		{#if p}
			<span class="pace" data-state={p.state} aria-hidden="true"
				>{p.glyph}{#if p.wall}<span class="wall">{p.wall}</span>{/if}</span
			>
		{/if}
	</span>
{/snippet}

{#if entries.length > 0}
	<span class="strip full">
		{#each groups as group (group[0].account)}
			<span class="group">
				{#each group as e (e.providerId)}
					{@const head = `${e.accountName} · ${providerLabel(e.provider)}`}
					{@const acct = accountOf(e.account)}
					<Popover
						label={m.usage_battery_aria({
							account: e.accountName,
							provider: providerLabel(e.provider),
							five: pctText(e.bars.fiveHour),
							weekly: pctText(e.bars.weekly)
						})}
						placement="bottom-end"
						bare
						hitArea="compact"
					>
						{#snippet trigger()}
							{@render cell(e.bars, titleOf(head, e.bars), e.provider)}
						{/snippet}
						<div class="panel">
							{#if acct}
								<AccountCard account={acct} compact />
							{:else}
								<Text size="sm" weight="semibold">{head}</Text>
							{/if}
						</div>
					</Popover>
				{/each}
			</span>
		{/each}
	</span>
	<span class="strip agg">
		<Popover
			label={m.usage_battery_aggregate_aria({
				five: pctText(agg.fiveHour),
				weekly: pctText(agg.weekly)
			})}
			placement="bottom-end"
			bare
			hitArea="compact"
		>
			{#snippet trigger()}
				{@render cell(agg, titleOf(m.usage_battery_aggregate_title(), agg), null)}
			{/snippet}
			<div class="panel">
				{#each groups as group (group[0].account)}
					{@const acct = accountOf(group[0].account)}
					{#if acct}
						<AccountCard account={acct} compact />
					{/if}
				{/each}
			</div>
		</Popover>
	</span>
{/if}

<style>
	/* Lives in the px-pinned header: every length is px, never rem. */
	.strip {
		align-items: center;
		gap: 6px;
		flex: none;
	}
	.full {
		display: inline-flex;
	}
	.agg {
		display: none;
	}
	@media (max-width: 639px) {
		.full {
			display: none;
		}
		.agg {
			display: inline-flex;
		}
	}
	.group {
		display: inline-flex;
		align-items: center;
		gap: 2px;
		padding: 0 2px;
		border-radius: 4px;
		border: 1px solid var(--border);
	}
	.glyph {
		display: inline-flex;
		flex: none;
	}
	.cell {
		display: inline-flex;
		align-items: center;
		gap: 3px;
		height: 24px;
		padding: 0 3px;
	}
	.bars {
		display: inline-flex;
		flex-direction: column;
		gap: 2px;
	}
	.track {
		display: block;
		width: 22px;
		height: 3px;
		border-radius: 2px;
		background: var(--border);
		overflow: hidden;
	}
	.track[data-tone='unknown'] {
		background: repeating-linear-gradient(45deg, var(--border-strong) 0 2px, transparent 2px 4px);
	}
	.fill {
		display: block;
		height: 100%;
		border-radius: 2px;
	}
	.track[data-tone='ok'] .fill {
		background: var(--ok);
	}
	.track[data-tone='warn'] .fill {
		background: var(--warn);
	}
	.track[data-tone='danger'] .fill {
		background: var(--danger);
	}
	.pace {
		display: inline-flex;
		align-items: center;
		gap: 2px;
		font-size: 10px;
		line-height: 1;
		white-space: nowrap;
	}
	.pace[data-state='neutral'] {
		color: var(--text-faint);
	}
	.pace[data-state='flame'] .wall {
		color: var(--danger);
		font-family: var(--font-mono, monospace);
		font-weight: 700;
	}
	.panel {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		width: 22rem;
		max-width: 90vw;
	}
</style>
