<script lang="ts">
	import type { Snippet } from 'svelte';
	import { Meter, Progress, type Tone } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { statTone, type MetricKind, type StatTone } from './tone';

	let {
		kind,
		value,
		label,
		size = 'full',
		caption
	}: {
		kind: MetricKind;
		/** Percent; `null` when nothing was reported. */
		value: number | null;
		label: string;
		size?: 'full' | 'light' | 'mini';
		caption?: string | Snippet;
	} = $props();

	const TSU_TONE: Record<StatTone, Tone> = {
		ok: 'ok',
		warn: 'warn',
		danger: 'danger',
		unknown: 'neutral'
	};
	const pct = $derived(value === null ? null : Math.max(0, Math.min(100, Math.round(value))));
	const tone = $derived(TSU_TONE[statTone(kind, pct)]);
	const readout = () => (pct === null ? m.usage_battery_unknown() : `${pct}%`);
</script>

{#if size === 'mini'}
	{#if pct === null}
		<span class="unknown" role="img" aria-label={`${label} ${readout()}`}></span>
	{:else}
		<!-- The header is px-pinned: the track keeps px lengths at every font scale. -->
		<Progress value={pct} {tone} {label} size="sm" shrink={false} style="width: 22px; height: 3px" />
	{/if}
{:else}
	<Meter
		{label}
		value={pct ?? 0}
		{tone}
		format={readout}
		{caption}
		size={size === 'light' ? 'sm' : 'md'}
	/>
{/if}

<style>
	.unknown {
		display: block;
		flex: none;
		width: 22px;
		height: 3px;
		border-radius: 2px;
		background: repeating-linear-gradient(45deg, var(--border-strong) 0 2px, transparent 2px 4px);
	}
</style>
