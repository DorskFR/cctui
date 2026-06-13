<script lang="ts">
	// Form field wrapper: a label above a slotted control, with optional hint and
	// error text. Replaces the ad-hoc `<div class="field"><label class="label">`
	// markup. Renders a real <label for> when `for` is given (associates with the
	// control), else a plain <span> for control groups (radio/segment rows).
	import type { Snippet } from 'svelte';

	let {
		label,
		for: forId,
		hint,
		error,
		class: klass = '',
		children
	}: {
		label?: string;
		for?: string;
		hint?: string;
		error?: string;
		class?: string;
		children?: Snippet;
	} = $props();
</script>

<div class="field {klass}">
	{#if label}
		{#if forId}
			<label class="label" for={forId}>{label}</label>
		{:else}
			<span class="label">{label}</span>
		{/if}
	{/if}
	{@render children?.()}
	{#if hint}<span class="hint">{hint}</span>{/if}
	{#if error}<span class="error">{error}</span>{/if}
</div>

<style>
	.field {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	.label {
		font-size: var(--fs-sm);
		font-weight: var(--fw-medium);
		color: var(--text-muted);
	}
	.hint {
		font-size: var(--fs-xs);
		color: var(--text-faint);
	}
	.error {
		font-size: var(--fs-xs);
		color: var(--danger);
	}
</style>
