<script lang="ts">
	import type { HTMLButtonAttributes } from 'svelte/elements';
	import Icon from './Icon.svelte';
	import type { IconName } from './Icon.svelte';

	type IconButtonProps = HTMLButtonAttributes & {
		icon: IconName;
		label: string;
		variant?: 'default' | 'primary' | 'ghost' | 'danger';
		size?: number;
		class?: string;
	};

	let {
		icon,
		label,
		title = label,
		variant = 'ghost',
		size = 16,
		disabled = false,
		onclick,
		class: klass = '',
		...rest
	}: IconButtonProps = $props();
</script>

<button
	{...rest}
	type="button"
	class="btn btn-icon {klass}"
	class:btn-primary={variant === 'primary'}
	class:btn-ghost={variant === 'ghost'}
	class:btn-danger={variant === 'danger'}
	aria-label={label}
	{title}
	{disabled}
	onclick={onclick}
>
	<Icon name={icon} {size} />
</button>
