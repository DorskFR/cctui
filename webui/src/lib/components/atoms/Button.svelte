<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { HTMLButtonAttributes } from 'svelte/elements';

	type ButtonProps = HTMLButtonAttributes & {
		variant?: 'default' | 'primary' | 'ghost' | 'danger';
		size?: 'sm' | 'md';
		control?: boolean;
		block?: boolean;
		class?: string;
		children?: Snippet;
	};

	let {
		variant = 'default',
		size = 'md',
		control = false,
		block = false,
		type = 'button',
		disabled = false,
		title,
		onclick,
		class: klass = '',
		children,
		...rest
	}: ButtonProps = $props();
</script>

<button
	{...rest}
	{type}
	{disabled}
	{title}
	class="btn {klass}"
	class:btn-primary={variant === 'primary'}
	class:btn-ghost={variant === 'ghost'}
	class:btn-danger={variant === 'danger'}
	class:btn-sm={size === 'sm'}
	class:btn-control={control}
	class:btn-block={block}
	onclick={onclick}
>
	{@render children?.()}
</button>

<style>
	/* The canonical control: owns its variants/sizes from theme tokens. Svelte 5
	   scopes via :where() (zero added specificity), so these match the same way
	   the global .btn rules did — feature-component overrides (e.g. .dhead
	   .tapbtn) keep winning by specificity. Icons size themselves (Icon.svelte),
	   so no svg sizing rule lives here. */
	.btn {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-4);
		min-height: 2.5rem;
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		background: var(--surface);
		color: var(--text);
		font-weight: var(--fw-medium);
		font-size: var(--fs-sm);
		line-height: 1;
		transition:
			background 0.12s var(--ease),
			border-color 0.12s var(--ease),
			opacity 0.12s var(--ease);
		user-select: none;
		white-space: nowrap;
	}
	.btn:hover:not(:disabled) {
		border-color: var(--accent);
	}
	.btn:disabled {
		opacity: 0.45;
		cursor: not-allowed;
	}
	.btn-primary {
		background: var(--accent);
		border-color: var(--accent);
		color: var(--text-on-accent);
		font-weight: var(--fw-semibold);
	}
	.btn-primary:hover:not(:disabled) {
		filter: brightness(1.08);
	}
	.btn-danger {
		color: var(--danger);
		border-color: color-mix(in srgb, var(--danger) 50%, var(--border));
	}
	.btn-danger:hover:not(:disabled) {
		background: color-mix(in srgb, var(--danger) 14%, transparent);
		border-color: var(--danger);
	}
	.btn-ghost {
		background: transparent;
		border-color: transparent;
	}
	.btn-ghost:hover:not(:disabled) {
		background: var(--bg-elevated-2);
		border-color: transparent;
	}
	.btn-sm {
		min-height: 2rem;
		padding: var(--sp-1) var(--sp-3);
		font-size: var(--fs-xs);
	}
	.btn-block {
		width: 100%;
	}
	/* Uniform-height control (CCT-250 item 1): icon buttons, inputs and action
	   buttons that share a toolbar/composer row line up exactly. */
	.btn-control {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: var(--sp-2);
		height: var(--control-height);
		min-height: var(--control-height);
		padding: 0 var(--sp-3);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		background: var(--surface);
		color: var(--text);
		font-weight: var(--fw-medium);
		font-size: var(--fs-sm);
		line-height: 1;
		white-space: nowrap;
		transition:
			background 0.12s var(--ease),
			border-color 0.12s var(--ease),
			opacity 0.12s var(--ease);
		user-select: none;
	}
	.btn-control:hover:not(:disabled) {
		border-color: var(--accent);
	}
	.btn-control:disabled {
		opacity: 0.45;
		cursor: not-allowed;
	}
	/* .btn-control follows .btn-primary in source order and would otherwise paint
	   the primary action with the neutral --surface; restore the accent fill when
	   both are present (CCT-345). */
	.btn-control.btn-primary {
		background: var(--accent);
		border-color: var(--accent);
		color: var(--text-on-accent);
	}
	.btn-control.btn-primary:hover:not(:disabled) {
		border-color: var(--accent);
		filter: brightness(1.08);
	}
</style>
