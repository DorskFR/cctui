<script lang="ts">
	// Canonical on/off toggle (role="switch") — a pill track with a sliding knob.
	// Owns its sizing/colors from theme tokens; the parent supplies `checked`
	// and an `onclick` handler plus an accessible label/title.
	import type { HTMLButtonAttributes } from 'svelte/elements';

	let {
		checked = false,
		label,
		class: klass = '',
		...rest
	}: HTMLButtonAttributes & { checked?: boolean; label: string } = $props();
</script>

<button
	{...rest}
	type="button"
	class="switch {klass}"
	class:on={checked}
	role="switch"
	aria-checked={checked}
	aria-label={label}
>
	<span class="knob"></span>
</button>

<style>
	.switch {
		flex: none;
		width: 2.75rem;
		height: 1.6rem;
		border-radius: var(--r-pill);
		border: 1px solid var(--border-strong);
		background: var(--bg-elevated-2);
		padding: 2px;
		display: flex;
		align-items: center;
		cursor: pointer;
		transition:
			background 0.14s var(--ease),
			border-color 0.14s var(--ease);
	}
	.switch .knob {
		width: 1.25rem;
		height: 1.25rem;
		border-radius: 50%;
		background: var(--text-muted);
		transition:
			transform 0.14s var(--ease),
			background 0.14s var(--ease);
	}
	.switch.on {
		background: var(--accent);
		border-color: var(--accent);
	}
	.switch.on .knob {
		transform: translateX(1.15rem);
		background: var(--text-on-accent);
	}
	.switch:disabled {
		opacity: 0.45;
		cursor: not-allowed;
	}
</style>
