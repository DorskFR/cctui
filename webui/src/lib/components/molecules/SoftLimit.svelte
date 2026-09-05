<script lang="ts">
	import { Input, Progress, Text, Timestamp } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import type { UsagePace } from '$lib/queries';
	import { countdown, paceState, wallInMs } from '$lib/components/molecules/UsageBattery.logic';

	// One usage window: its label + utilization bar + percent + reset +
	// configured-cap marker, optionally with controls to set/clear the window's
	// cap% and bypass minutes. The SAME component renders every window both in the
	// read-only usage view and in the account editor — behaviour keyed off props,
	// never a hardcoded window list.
	let {
		label,
		utilization = null,
		amountUsd = null,
		resets = null,
		cap = $bindable(null),
		capUsd = $bindable(null),
		bypass = $bindable(null),
		editable = false,
		observed = true,
		usd = false,
		pace = null
	}: {
		label: string;
		/** 0–100 (may exceed); null ⇒ not currently reported. */
		utilization?: number | null;
		/** USD spent, for a dollar window. */
		amountUsd?: number | null;
		resets?: string | null;
		cap?: number | null;
		capUsd?: number | null;
		bypass?: number | null;
		editable?: boolean;
		observed?: boolean;
		/** Dollar window: spend against a $ cap instead of % of a quota. */
		usd?: boolean;
		/** Server burn rate: draws the expected-now marker and the pace glyph. */
		pace?: UsagePace | null;
	} = $props();

	// Severity breakpoints on window utilization (%): below WARN → success,
	// WARN–HOT → warn, at/above HOT → danger. Keeps bar + percent in lockstep.
	const WARN_PCT = 70;
	const HOT_PCT = 90;

	// A dollar window has no quota to be a percentage of, so its bar is spend
	// against its own cap; with no cap there is nothing to fill.
	const usdPct = $derived(
		amountUsd === null || capUsd == null || capUsd <= 0
			? null
			: Math.max(0, Math.min(100, Math.round((amountUsd / capUsd) * 100)))
	);
	const pct = $derived(
		usd
			? usdPct
			: utilization === null
				? null
				: Math.max(0, Math.min(100, Math.round(utilization)))
	);
	const money = (n: number) => `$${n.toFixed(2)}`;
	const tone = $derived(
		pct === null ? 'success' : pct >= HOT_PCT ? 'danger' : pct >= WARN_PCT ? 'warn' : 'success'
	);
	const capPct = $derived(cap != null && cap >= 0 && cap <= 100 ? cap : null);

	const state = $derived(usd ? null : paceState(pace));
	const expectedPct = $derived(
		state && pace ? Math.max(0, Math.min(100, Math.round(pace.expected_pct))) : null
	);
	const wallMs = $derived(state ? wallInMs(pace, resets, Date.now()) : null);
	const paceGlyph = $derived(state === 'leaf' ? '🍃' : state === 'flame' ? '🔥' : '•');
	const paceTitle = $derived.by(() => {
		if (!state || expectedPct === null) return '';
		const parts = [
			state === 'leaf'
				? m.usage_battery_pace_leaf()
				: state === 'flame'
					? m.usage_battery_pace_flame()
					: m.usage_battery_pace_neutral(),
			m.usage_battery_pace_expected({ expected: expectedPct })
		];
		if (wallMs !== null) parts.push(m.usage_battery_pace_wall({ time: countdown(wallMs) }));
		return parts.join(' · ');
	});
</script>

<div class="soft-limit">
	<div class="head">
		<Text size="xs" tone="muted" numeric class="sl-label">{label}</Text>
		{#if usd && amountUsd !== null}
			<Text size="xs" numeric tone={tone === 'danger' ? 'danger' : tone === 'warn' ? 'warn' : 'success'} class="sl-pct">
				{money(amountUsd)}{#if capUsd != null}<Text tone="faint"> / {money(capUsd)}</Text>{/if}{#if resets}<Text tone="faint" class="sl-reset"> · resets <Timestamp value={resets} mode="relative" tone="faint" /></Text>{/if}
			</Text>
		{:else if pct !== null}
			<Text size="xs" numeric tone={tone === 'danger' ? 'danger' : tone === 'warn' ? 'warn' : 'success'} class="sl-pct">
				{pct}%{#if resets}<Text tone="faint" class="sl-reset"> · resets <Timestamp value={resets} mode="relative" tone="faint" /></Text>{/if}{#if state}<Text as="span" tone={state === 'flame' ? 'danger' : state === 'leaf' ? 'success' : 'faint'} title={paceTitle} aria-label={paceTitle} class="sl-pace"> {paceGlyph}{#if wallMs !== null} {countdown(wallMs)}{/if}</Text>{/if}
			</Text>
		{:else}
			<Text size="xs" tone="faint" class="sl-pct">{m.softlimit_not_reported()}</Text>
		{/if}
	</div>

	{#if pct !== null}
		<div class="track-wrap">
			<Progress value={pct} label={m.sessions_usage_bar_aria({ label })} tone={tone} class="sl-track" />
			{#if !usd && capPct != null}
				<span class="cap-marker" style={`left: ${capPct}%`} title={m.sessions_usage_soft_limit({ pct: capPct })}></span>
			{/if}
			{#if expectedPct !== null}
				<span class="pace-marker" style={`left: ${expectedPct}%`} title={m.usage_battery_pace_marker({ pct: expectedPct })}></span>
			{/if}
		</div>
	{/if}

	{#if editable}
		<div class="controls">
			<label class="ctrl">
				{#if usd}
					<Text as="div" tone="faint" size="xs">{m.softlimit_cap_usd_label()}</Text>
					<Input type="number" step="0.01" bind:value={capUsd} placeholder="e.g. 5.00" />
				{:else}
					<Text as="div" tone="faint" size="xs">{m.softlimit_cap_label()}</Text>
					<Input type="number" bind:value={cap} placeholder="e.g. 80" />
				{/if}
			</label>
			<label class="ctrl">
				<Text as="div" tone="faint" size="xs">{m.softlimit_bypass_label()}</Text>
				<Input type="number" bind:value={bypass} placeholder="e.g. 30" />
			</label>
		</div>
	{/if}
</div>

<style>
	.soft-limit {
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
		min-width: 0;
	}
	.head {
		display: grid;
		grid-template-columns: auto 1fr;
		align-items: baseline;
		column-gap: var(--sp-2);
	}
	.soft-limit :global(.sl-pct) {
		justify-self: end;
	}
	.track-wrap {
		position: relative;
		margin-top: 0.15rem;
	}
	/* Soft-limit cap marker: a thin vertical line at the
	   configured % so the ceiling reads against the live fill. */
	.cap-marker {
		position: absolute;
		top: -1px;
		bottom: -1px;
		width: 2px;
		transform: translateX(-1px);
		background: var(--text-muted, currentColor);
		opacity: 0.8;
		pointer-events: none;
		border-radius: 1px;
	}
	/* Expected-now marker: where an even spend would sit, so the fill reads
	   as ahead of or behind pace at a glance. */
	.pace-marker {
		position: absolute;
		top: -3px;
		bottom: -3px;
		width: 1px;
		background: var(--accent);
		opacity: 0.9;
		pointer-events: none;
	}
	.controls {
		display: grid;
		grid-template-columns: repeat(2, 1fr);
		gap: var(--sp-2);
		margin-top: var(--sp-1);
	}
	.ctrl {
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
		min-width: 0;
	}
</style>
