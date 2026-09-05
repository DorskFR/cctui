<script lang="ts">
	// Inline error banner: the single home for "something went wrong" copy under
	// a field or panel. A danger Callout, so it announces as an alert.
	import type { Snippet } from 'svelte';
	import { Callout } from '@dorsk/tsumikit';

	let { children, ...rest }: { children?: Snippet; [key: string]: unknown } = $props();
</script>

<Callout tone="danger" {...rest}>{@render children?.()}</Callout>
