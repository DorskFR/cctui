<script lang="ts">
	import { CapBar, Input, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import type { UsagePace } from '$lib/queries';
	import { countdown, paceState, wallInMs } from '$lib/components/molecules/header-gauges.logic';
	import { capFromBar, capToBar, resetIn, usdPct, usdReadout } from './cap-bar.logic';

	// One usage window as a cap bar: consumption fill, draggable cap, readout.
	// The same component renders the read-only usage view (`oncapchange`
	// commits the drag) and the editor (`editable` adds the bypass input).
	let {
		label,
		utilization = null,
		amountUsd = null,
		resets = null,
		cap = $bindable(null),
		capUsd = $bindable(null),
		bypass = $bindable(null),
		editable = false,
		usd = false,
		pace = null,
		oncapchange
	}: {
		label: string;
		/** 0–100 (may exceed); null ⇒ not currently reported. */
		utilization?: number | null;
		amountUsd?: number | null;
		resets?: string | null;
		cap?: number | null;
		capUsd?: number | null;
		bypass?: number | null;
		editable?: boolean;
		/** Dollar window: spend against a $ cap instead of % of a quota. */
		usd?: boolean;
		pace?: UsagePace | null;
		/** Commits a dragged cap; absent (and not `editable`) ⇒ the bar is read-only. */
		oncapchange?: (cap: number | null) => void;
	} = $props();

	const pct = $derived(
		usd
			? usdPct(amountUsd, capUsd)
			: utilization === null
				? null
				: Math.max(0, Math.min(100, Math.round(utilization)))
	);
	const reported = $derived(usd ? amountUsd !== null : utilization !== null);
	const now = Date.now();
	const readout = $derived.by(() => {
		if (usd) return usdReadout(amountUsd, capUsd) ?? m.softlimit_not_reported();
		if (pct === null) return m.softlimit_not_reported();
		const time = resetIn(resets, now);
		return time ? m.capbar_readout_resets({ pct, time }) : `${pct}%`;
	});
	const readonly = $derived(usd || (!editable && !oncapchange));

	// The three-column bar (label | track | readout) only fits while the readout
	// column can hold "100 % · resets 5d 12h" next to a track worth looking at.
	// Below that the row goes dense: the window name takes its own line above a
	// full-width track, so neither the name nor the reset countdown is clipped.
	// Measured rather than a media query — the stats panel is drag-resizable.
	const READOUT_W = '9.5rem';
	const DENSE_BELOW_PX = 340;
	let width = $state(0);
	const dense = $derived(width > 0 && width < DENSE_BELOW_PX);

	let barCap = $derived(capToBar(cap));
	function commit(next: number) {
		const value = capFromBar(next);
		cap = value;
		oncapchange?.(value);
	}

	const paceKind = $derived(usd ? null : paceState(pace));
	const expectedPct = $derived(
		paceKind && pace ? Math.max(0, Math.min(100, Math.round(pace.expected_pct))) : null
	);
	const wallMs = $derived(paceKind ? wallInMs(pace, resets, now) : null);
	const paceGlyph = $derived(paceKind === 'leaf' ? '🍃' : paceKind === 'flame' ? '🔥' : '•');
	const paceTitle = $derived.by(() => {
		if (!paceKind || expectedPct === null) return '';
		const parts = [
			paceKind === 'leaf'
				? m.usage_battery_pace_leaf()
				: paceKind === 'flame'
					? m.usage_battery_pace_flame()
					: m.usage_battery_pace_neutral(),
			m.usage_battery_pace_expected({ expected: expectedPct })
		];
		if (wallMs !== null) parts.push(m.usage_battery_pace_wall({ time: countdown(wallMs) }));
		return parts.join(' · ');
	});
</script>

<div class="soft-limit" bind:clientWidth={width}>
	{#if dense}
		<div class="dense-label"><Text size="xs" tone="muted" truncate>{label}</Text></div>
	{/if}
	<CapBar
		label={dense ? undefined : label}
		value={pct ?? 0}
		bind:cap={barCap}
		step={5}
		warnAt={75}
		labelWidth={dense ? '0px' : '96px'}
		readoutWidth={reported ? (dense ? 'max-content' : READOUT_W) : 'auto'}
		{readout}
		{readonly}
		tooltip={readonly ? m.capbar_tooltip_readonly({ pct: barCap }) : m.capbar_tooltip({ pct: barCap })}
		onchange={commit}
	>
		{#snippet caption()}
			{#if paceKind}
				<Text
					as="span"
					size="xs"
					tone={paceKind === 'flame' ? 'danger' : paceKind === 'leaf' ? 'success' : 'faint'}
					title={paceTitle}
					aria-label={paceTitle}
				>
					{paceGlyph}{#if wallMs !== null} {countdown(wallMs)}{/if}
					{#if expectedPct !== null}· {m.usage_battery_pace_marker({ pct: expectedPct })}{/if}
				</Text>
			{/if}
		{/snippet}
	</CapBar>

	{#if editable}
		<div class="controls">
			{#if usd}
				<label class="ctrl">
					<Text as="div" tone="faint" size="xs">{m.softlimit_cap_usd_label()}</Text>
					<Input type="number" step="0.01" bind:value={capUsd} placeholder="e.g. 5.00" />
				</label>
			{/if}
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
		gap: var(--sp-1);
		min-width: 0;
	}
	.dense-label {
		min-width: 0;
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
		gap: var(--sp-1);
		min-width: 0;
	}
</style>
