<script lang="ts">
	// The composer's Send split-button. Stays a plain primary button across all
	// cost states: a `tone` on `primary` recolors the label over the accent fill
	// (unreadable), so the cold/imminent state lives in the label and the title.
	import { Button, SplitButton, type MenuItem } from '@dorsk/tsumikit';
	import { compact } from '$lib/format';
	import type { EnterRoute } from '$lib/followup';
	import { m } from '$lib/paraglide/messages';

	let {
		items,
		disabled,
		uploading,
		cacheCold,
		coldImminent,
		coldCountdownSecs,
		burstTokens,
		route = 'send',
		launching = false,
		menu = true,
		onclick
	}: {
		items: MenuItem[];
		disabled: boolean;
		uploading: boolean;
		cacheCold: boolean;
		coldImminent: boolean;
		coldCountdownSecs: number | null;
		burstTokens: number | null;
		/** Where a click goes; anything but `send` labels the follow-up. */
		route?: EnterRoute;
		launching?: boolean;
		/** False when nothing could go in the caret menu: a plain button then. */
		menu?: boolean;
		onclick: () => void;
	} = $props();

	const followup = $derived(cacheCold && route !== 'send');
	const coldTitle = $derived(
		burstTokens
			? m.composer_cache_cold_burst({ tokens: compact(burstTokens) })
			: m.composer_cache_cold()
	);
	const title = $derived(
		followup
			? route === 'launch'
				? m.composer_followup_auto_title()
				: m.composer_followup_modal_title()
			: cacheCold
				? coldTitle
				: coldImminent
					? m.composer_cache_imminent()
					: undefined
	);
	const verb = $derived(followup ? m.composer_followup_send() : m.composer_send());
</script>

{#snippet sendLabel()}
	{#if uploading}{m.composer_uploading()}{:else if launching}{m.composer_followup_launching()}{:else if coldImminent}{m.composer_send()} (<span
			class="countdown">{coldCountdownSecs}s</span
		>){:else if cacheCold && burstTokens}{verb} ❄️ ~{compact(burstTokens)}{:else if cacheCold}{verb}
		❄️{:else}{m.composer_send()}{/if}
{/snippet}

{#if menu}
	<SplitButton
		variant="primary"
		size="sm"
		caret="half"
		label={m.composer_schedule_menu()}
		{items}
		placement="top-end"
		disabled={disabled || launching}
		{onclick}
		{title}
	>
		{@render sendLabel()}
	</SplitButton>
{:else}
	<Button variant="primary" size="sm" disabled={disabled || launching} {onclick} {title}>
		{@render sendLabel()}
	</Button>
{/if}

<style>
	/* Fixed-width, tabular digits so "59s"→"0s" doesn't jitter the button. */
	.countdown {
		display: inline-block;
		min-width: 2.4ch;
		text-align: right;
		font-variant-numeric: tabular-nums;
	}
</style>
