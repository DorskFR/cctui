<script lang="ts">
	import type { Snippet } from 'svelte';

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
		children
	}: {
		variant?: 'default' | 'primary' | 'ghost' | 'danger';
		size?: 'sm' | 'md';
		control?: boolean;
		block?: boolean;
		type?: 'button' | 'submit' | 'reset';
		disabled?: boolean;
		title?: string;
		onclick?: (e: MouseEvent) => void;
		class?: string;
		children?: Snippet;
	} = $props();
</script>

<button
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
