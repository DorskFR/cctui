<script lang="ts">
	import type { HTMLButtonAttributes } from 'svelte/elements';
	import Button from '$lib/components/atoms/Button.svelte';
	import Icon from '$lib/components/atoms/Icon.svelte';
	import type { IconName } from '$lib/components/atoms/Icon.svelte';

	type IconButtonProps = HTMLButtonAttributes & {
		icon: IconName;
		label: string;
		variant?: 'default' | 'primary' | 'ghost' | 'danger';
		size?: number;
		// Borderless, compact icon affordance (chip-remove ✕, inline edit ✎) —
		// no square `.btn-icon` box; just a muted glyph that brightens on hover.
		inline?: boolean;
		class?: string;
	};

	let {
		icon,
		label,
		title = label,
		variant = 'ghost',
		size = 16,
		inline = false,
		disabled = false,
		onclick,
		class: klass = '',
		...rest
	}: IconButtonProps = $props();
</script>

<!-- Composition: the icon-only button is a Button (canonical control styling)
     carrying the square `.btn-icon` modifier, wrapping an Icon. -->
<Button {...rest} {variant} {disabled} {title} {onclick} class="{inline ? 'btn-icon-inline' : 'btn-icon'} {klass}" aria-label={label}>
	<Icon name={icon} {size} />
</Button>
