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
