<script lang="ts">
	import Icon from './Icon.svelte';

	let {
		icon,
		label,
		title = label,
		variant = 'ghost',
		size = 16,
		disabled = false,
		onclick,
		class: klass = ''
	}: {
		icon: 'search' | 'archive' | 'download' | 'link' | 'markdown' | 'image' | 'fork' | 'stop';
		label: string;
		title?: string;
		variant?: 'default' | 'primary' | 'ghost' | 'danger';
		size?: number;
		disabled?: boolean;
		onclick?: (e: MouseEvent) => void;
		class?: string;
	} = $props();
</script>

<button
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
