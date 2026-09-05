<script lang="ts">
	import { CapBar, Input, Text, Timestamp } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import type { UsagePace } from '$lib/queries';
	import { countdown, paceState, wallInMs } from '$lib/components/molecules/usage-battery.logic';
	import { capFromBar, capToBar, resetIn, resetInShort, usdPct, usdReadout } from './cap-bar.logic';

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
	const resetText = $derived(usd || pct === null ? null : resetIn(resets, now));
	const resetShort = $derived(usd || pct === null ? null : resetInShort(resets, now));
	const readonly = $derived(usd || (!editable && !oncapchange));

	// One line per window: label | track | "69% · in 3h" with a live relative
	// Timestamp. Pace and the full countdown live in the row's tooltip; only a
	// burn (flame) earns a glyph.
	const readoutText = $derived.by(() => {
		if (usd) return usdReadout(amountUsd, capUsd) ?? m.softlimit_not_reported();
		if (pct === null) return m.softlimit_not_reported();
		return `${pct}%`;
	});
	const showReset = $derived(!usd && pct !== null && resetShort !== null);

	// The three-column bar only fits while the readout column can hold
	// "100% · resets 5d 🔥" next to a track worth looking at. Below that the
	// window name takes its own line above a full-width track. Measured rather
	// than a media query — the stats panel is drag-resizable.
	const READOUT_W = $derived(usd ? '7.5rem' : '6rem');
	const DENSE_BELOW_PX = 300;
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
	const rowTitle = $derived.by(() => {
		const parts: string[] = [];
		if (resetText) parts.push(m.capbar_caption_resets({ time: resetText }));
		if (paceKind && expectedPct !== null) {
			parts.push(
				paceKind === 'leaf'
					? m.usage_battery_pace_leaf()
					: paceKind === 'flame'
						? m.usage_battery_pace_flame()
						: m.usage_battery_pace_neutral(),
				m.usage_battery_pace_expected({ expected: expectedPct })
			);
			if (wallMs !== null) parts.push(m.usage_battery_pace_wall({ time: countdown(wallMs) }));
		}
		return parts.join(' · ');
	});
</script>

{#snippet readoutSnippet()}
	<span class="readout">
		{readoutText}{#if showReset && resets}{' ·\u00a0'}<Timestamp
				value={resets}
				mode="relative"
				tone="inherit"
			/>{/if}{#if paceKind === 'flame'}{' 🔥'}{/if}
	</span>
{/snippet}

<div class="soft-limit" bind:clientWidth={width} title={rowTitle || undefined}>
	{#if dense}
		<div class="dense-label"><Text size="xs" tone="muted" truncate>{label}</Text></div>
	{/if}
	<CapBar
		label={dense ? undefined : label}
		value={pct ?? 0}
		bind:cap={barCap}
		step={5}
		warnAt={75}
		labelWidth={dense ? '0px' : '4rem'}
		readoutWidth={reported ? (dense ? 'max-content' : READOUT_W) : 'auto'}
		readout={showReset || paceKind === 'flame' ? readoutSnippet : readoutText}
		{readonly}
		tooltip={readonly ? m.capbar_tooltip_readonly({ pct: barCap }) : m.capbar_tooltip({ pct: barCap })}
		onchange={commit}
	/>

	{#if editable}
		<div class="controls">
			{#if usd}
				<label class="ctrl">
					<Text as="span" tone="faint" size="xs">{m.softlimit_cap_usd_label()}</Text>
					<Input type="number" step="0.01" size="sm" mono width="6rem" bind:value={capUsd} placeholder="e.g. 5.00" />
				</label>
			{/if}
			<label class="ctrl">
				<Text as="span" tone="faint" size="xs">{m.softlimit_bypass_label()}</Text>
				<Input type="number" size="sm" mono width="5rem" bind:value={bypass} placeholder="e.g. 30" />
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
	.readout {
		display: inline-flex;
		align-items: center;
		white-space: nowrap;
	}
	/* The editor's fields ride one line under the bar, flush right. */
	.controls {
		display: flex;
		justify-content: flex-end;
		gap: var(--sp-3);
	}
	.ctrl {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
</style>
