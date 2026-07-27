<script lang="ts">
	import { Input, Progress, Text, Timestamp } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	// One usage window: its label + utilization bar + percent + reset +
	// configured-cap marker, optionally with controls to set/clear the window's
	// cap% and bypass minutes. The SAME component renders every window both in the
	// read-only usage view and in the account editor — behaviour keyed off props,
	// never a hardcoded window list.
	let {
		label,
		utilization = null,
		resets = null,
		cap = $bindable(null),
		bypass = $bindable(null),
		editable = false,
		observed = true
	}: {
		label: string;
		/** 0–100 (may exceed); null ⇒ not currently reported. */
		utilization?: number | null;
		resets?: string | null;
		cap?: number | null;
		bypass?: number | null;
		editable?: boolean;
		observed?: boolean;
	} = $props();

	// Severity breakpoints on window utilization (%): below WARN → success,
	// WARN–HOT → warn, at/above HOT → danger. Keeps bar + percent in lockstep.
	const WARN_PCT = 70;
	const HOT_PCT = 90;

	const pct = $derived(
		utilization === null ? null : Math.max(0, Math.min(100, Math.round(utilization)))
	);
	const tone = $derived(
		pct === null ? 'success' : pct >= HOT_PCT ? 'danger' : pct >= WARN_PCT ? 'warn' : 'success'
	);
	const capPct = $derived(cap != null && cap >= 0 && cap <= 100 ? cap : null);
</script>

<div class="soft-limit">
	<div class="head">
		<Text size="xs" tone="muted" numeric class="sl-label">{label}</Text>
		{#if pct !== null}
			<Text size="xs" numeric tone={tone === 'danger' ? 'danger' : tone === 'warn' ? 'warn' : 'success'} class="sl-pct">
				{pct}%{#if resets}<Text tone="faint" class="sl-reset"> · resets <Timestamp value={resets} mode="relative" tone="faint" /></Text>{/if}
			</Text>
		{:else}
			<Text size="xs" tone="faint" class="sl-pct">{m.softlimit_not_reported()}</Text>
		{/if}
	</div>

	{#if pct !== null}
		<div class="track-wrap">
			<Progress value={pct} label={m.sessions_usage_bar_aria({ label })} tone={tone} class="sl-track" />
			{#if capPct != null}
				<span class="cap-marker" style={`left: ${capPct}%`} title={m.sessions_usage_soft_limit({ pct: capPct })}></span>
			{/if}
		</div>
	{/if}

	{#if editable}
		<div class="controls">
			<label class="ctrl">
				<Text as="div" tone="faint" size="xs">{m.softlimit_cap_label()}</Text>
				<Input type="number" bind:value={cap} placeholder="e.g. 80" />
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
