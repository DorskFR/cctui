<script lang="ts">
	// Shared edit/create modal for the "name (+ optional badge colour)" flows on
	// the users page: user rename, machine rename+recolour, token
	// (re)label. Replaces the native prompt()/confirm() dialogs. One molecule,
	// adapted per call-site via props — set `color` to surface the hue palette
	// (machines), leave it off for plain name/label edits.
	import { untrack } from 'svelte';
	import { Button, Field, Input, Modal } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import Swatch from '$lib/components/atoms/Swatch.svelte';

	let {
		title,
		fieldLabel = m.users_field_name_default(),
		name: initialName = '',
		placeholder = '',
		hint,
		saveLabel = m.common_save(),
		color = false,
		hue: initialHue = null,
		hues = [],
		onsave,
		onclose
	}: {
		title: string;
		fieldLabel?: string;
		name?: string | null;
		placeholder?: string;
		hint?: string;
		saveLabel?: string;
		/** Show the badge-colour palette (machines). */
		color?: boolean;
		hue?: number | null;
		hues?: number[];
		/** name is the trimmed value or null when empty (unlabeled). */
		onsave: (name: string | null, hue: number | null) => void;
		onclose: () => void;
	} = $props();

	// Freshly mounted per open (keyed by the caller's {#if}), so capturing the
	// initial prop values once is intended — untrack documents that.
	let name = $state(untrack(() => initialName) ?? '');
	let hue = $state<number | null>(untrack(() => initialHue));
	let inputEl = $state<HTMLInputElement | null>(null);

	$effect(() => {
		inputEl?.focus();
		inputEl?.select();
	});

	function save() {
		onsave(name.trim() || null, hue);
		onclose();
	}
</script>

<Modal {title} {onclose} size="sm">
	{#snippet body()}
		<form
			class="stack body"
			onsubmit={(e) => {
				e.preventDefault();
				save();
			}}
		>
			<Field label={fieldLabel} for="edit-name" {hint}>
				<Input id="edit-name" bind:value={name} bind:el={inputEl} {placeholder} />
			</Field>
			{#if color}
				<Field label={m.users_badge_colour()}>
					<div class="row palette" role="radiogroup" aria-label={m.users_badge_colour()}>
						<Swatch
							hue={null}
							active={hue == null}
							title={m.users_colour_auto_title()}
							aria-label={m.users_colour_auto_aria()}
							onclick={() => (hue = null)}>A</Swatch
						>
						{#each hues as h (h)}
							<Swatch
								hue={h}
								active={hue === h}
								title={m.users_hue({ hue: h })}
								aria-label={m.users_hue({ hue: h })}
								onclick={() => (hue = h)}
							/>
						{/each}
					</div>
				</Field>
			{/if}
		</form>
	{/snippet}
	{#snippet footer()}
		<Button block onclick={onclose}>{m.common_cancel()}</Button>
		<Button block variant="primary" onclick={save}>{saveLabel}</Button>
	{/snippet}
</Modal>

<style>
	.body {
		gap: var(--sp-3);
	}
	.palette {
		flex-wrap: wrap;
		gap: var(--sp-2);
		align-items: center;
	}
</style>
