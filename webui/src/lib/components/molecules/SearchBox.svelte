<script lang="ts">
	import { Input, IconButton } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	// The sessions toolbar search field: a control-height search input with an
	// in-field clear cross (CCT-297 #22). `value` is bindable so the parent owns
	// the query (and its debounce); everything else — focus handling, the clear
	// button — lives here.
	let {
		value = $bindable(''),
		placeholder = m.misc_search_all_chats_placeholder()
	}: { value?: string; placeholder?: string } = $props();

	let el = $state<HTMLInputElement | null>(null);
</script>

<div class="search-box">
	<Input
		class="search"
		type="search"
		{placeholder}
		bind:value
		bind:el
		onkeydown={(e) => {
			if (e.key === 'Escape') {
				value = '';
				(e.currentTarget as HTMLInputElement).blur();
			}
		}}
	/>
	{#if value}
		<!-- onmousedown preventDefault keeps the input from blurring before the
		     click clears the query. -->
		<IconButton
			inline
			class="search-clear"
			icon="x"
			size={13}
			label={m.misc_clear_search()}
			title={m.misc_clear_search()}
			onmousedown={(e: MouseEvent) => e.preventDefault()}
			onclick={() => {
				value = '';
				el?.focus();
			}}
		/>
	{/if}
</div>

<style>
	/* The wrapper is the flex item + positioning context for the in-field clear. */
	.search-box {
		min-width: 0;
		position: relative;
		display: flex;
	}
	/* Toolbar tweaks layered on the Input atom (`.input.search` beats the atom's
	   `.input` base): control height, elevated fill, and right padding clearing
	   the × button. */
	.search-box :global(.input.search) {
		flex: 1;
		min-width: 0;
		height: var(--control-height);
		padding: var(--sp-1) calc(var(--sp-3) + 1.25rem) var(--sp-1) var(--sp-3);
		font-size: var(--fs-sm);
		background: var(--bg-elevated);
	}
	/* Hide the browser-native search clear; we provide our own cross. */
	.search-box :global(.input.search)::-webkit-search-cancel-button {
		display: none;
	}
	.search-box :global(.search-clear) {
		position: absolute;
		top: 50%;
		right: var(--sp-2);
		transform: translateY(-50%);
		width: 1.25rem;
		height: 1.25rem;
		padding: 0;
		border-radius: 50%;
		background: var(--bg-elevated-2, var(--border));
		color: var(--text-faint);
	}
	.search-box :global(.search-clear):hover {
		color: var(--text);
		background: var(--bg-elevated-2, var(--border));
	}
</style>
