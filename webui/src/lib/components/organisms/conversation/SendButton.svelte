<script lang="ts">
	// The composer's Send split-button. Stays a plain primary button across all
	// cost states: a `tone` on `primary` recolors the label over the accent fill
	// (unreadable), so the cold/imminent state lives in the label and the title.
	import { SplitButton, type MenuItem } from '@dorsk/tsumikit';
	import { compact } from '$lib/format';
	import { m } from '$lib/paraglide/messages';

	let {
		items,
		disabled,
		uploading,
		cacheCold,
		coldImminent,
		coldCountdownSecs,
		burstTokens,
		onclick
	}: {
		items: MenuItem[];
		disabled: boolean;
		uploading: boolean;
		cacheCold: boolean;
		coldImminent: boolean;
		coldCountdownSecs: number | null;
		burstTokens: number | null;
		onclick: () => void;
	} = $props();
</script>

<SplitButton
	variant="primary"
	size="sm"
	label={m.composer_schedule_menu()}
	{items}
	placement="top-end"
	{disabled}
	{onclick}
	title={cacheCold
		? burstTokens
			? m.composer_cache_cold_burst({ tokens: compact(burstTokens) })
			: m.composer_cache_cold()
		: coldImminent
			? m.composer_cache_imminent()
			: undefined}
>
	{#if uploading}{m.composer_uploading()}{:else if coldImminent}{m.composer_send()} (<span
			class="countdown">{coldCountdownSecs}s</span
		>){:else if cacheCold && burstTokens}{m.composer_send()} ❄️ ~{compact(
			burstTokens
		)}{:else if cacheCold}{m.composer_send()}
		❄️{:else}{m.composer_send()}{/if}
</SplitButton>

<style>
	/* Fixed-width, tabular digits so "59s"→"0s" doesn't jitter the button. */
	.countdown {
		display: inline-block;
		min-width: 2.4ch;
		text-align: right;
		font-variant-numeric: tabular-nums;
	}
</style>
