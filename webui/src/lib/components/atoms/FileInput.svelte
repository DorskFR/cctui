<script lang="ts">
	// File-picker primitive: the only place <input type="file"> is emitted. The
	// default is a visible control with a real, thumb-sized browse button (the UA
	// default is tiny/untappable on mobile — CCT-241). `hidden` renders it visually
	// hidden for the "icon label wraps the input" pattern (composer 📎 button).
	import type { HTMLInputAttributes } from 'svelte/elements';

	type Props = HTMLInputAttributes & {
		hidden?: boolean;
		class?: string;
	};

	let { hidden = false, class: klass = '', ...rest }: Props = $props();
</script>

<input type="file" class="fileinput {hidden ? 'sr' : ''} {klass}" {...rest} />

<style>
	.fileinput {
		font-size: var(--fs-xs);
		color: var(--text-muted);
	}
	.fileinput::file-selector-button {
		padding: var(--sp-2) var(--sp-4);
		margin-right: var(--sp-3);
		font-size: var(--fs-sm);
		font-weight: var(--fw-medium);
		color: var(--text);
		background: var(--bg-elevated-2);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		cursor: pointer;
	}
	.fileinput::file-selector-button:hover {
		border-color: var(--c-blue);
	}
	/* Visually hidden: the wrapping label/button is the visible affordance. */
	.fileinput.sr {
		position: absolute;
		width: 1px;
		height: 1px;
		opacity: 0;
		pointer-events: none;
	}
</style>
