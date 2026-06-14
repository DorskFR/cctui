<script lang="ts">
	// Inline error / validation message primitive. The single home for "red text
	// that says something went wrong" — replaces the per-component `:global(.err)`
	// reach-ins that each restyled a Text child (and collided in the global sheet,
	// both being named `.err`). Owns the danger tone via tsumikit's `tone` prop, so
	// there's no scoped-CSS leak. Defaults to xs; pass `size` to override.
	import type { Snippet } from 'svelte';
	import { Text } from '@dorsk/tsumikit';

	// Mirrors tsumikit Text's Size union (not exported, so inlined here).
	type Size = 'xs' | 'sm' | 'base' | 'md' | 'lg' | 'xl' | '2xl';

	let {
		size = 'xs',
		children,
		...rest
	}: { size?: Size; children?: Snippet; [key: string]: unknown } = $props();
</script>

<Text {size} tone="danger" role="alert" {...rest}>{@render children?.()}</Text>
