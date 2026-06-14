<script lang="ts">
	// Shared edit/create modal for the "name (+ optional badge colour)" flows on
	// the users page (CCT-301): user rename, machine rename+recolour, token
	// (re)label. Replaces the native prompt()/confirm() dialogs. One molecule,
	// adapted per call-site via props — set `color` to surface the hue palette
	// (machines), leave it off for plain name/label edits.
	import { untrack } from 'svelte';
	import { Button, Field, Input, Modal } from '@dorsk/tsumikit';
	import Swatch from '$lib/components/atoms/Swatch.svelte';

	let {
		title,
		fieldLabel = 'Name',
		name: initialName = '',
		placeholder = '',
		hint,
		saveLabel = 'Save',
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
				<Field label="Badge colour">
					<div class="row palette" role="radiogroup" aria-label="Badge colour">
						<Swatch
							hue={null}
							active={hue == null}
							title="Auto (name hash)"
							aria-label="Auto colour"
							onclick={() => (hue = null)}>A</Swatch
						>
						{#each hues as h (h)}
							<Swatch
								hue={h}
								active={hue === h}
								title={`Hue ${h}`}
								aria-label={`Hue ${h}`}
								onclick={() => (hue = h)}
							/>
						{/each}
					</div>
				</Field>
			{/if}
		</form>
	{/snippet}
	{#snippet footer()}
		<Button block onclick={onclose}>Cancel</Button>
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
