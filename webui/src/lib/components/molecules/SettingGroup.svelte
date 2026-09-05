<script lang="ts">
	// A bordered box of SettingRows, with an optional small-caps header naming
	// the sub-topic (List · New session · Conversation …). Rows draw their own
	// hairlines; the box only supplies the surface, the header and the corner
	// radius. `data-setting-group` lets the page filter hide an emptied box.
	import type { Snippet } from 'svelte';
	import { Text } from '@dorsk/tsumikit';

	let { title, children }: { title?: string; children?: Snippet } = $props();
</script>

<div class="group" data-setting-group>
	{#if title}
		<div class="group-h">
			<Text size="xs" weight="semibold" tone="faint" as="div">{title}</Text>
		</div>
	{/if}
	<div class="rows">{@render children?.()}</div>
</div>

<style>
	.group {
		background: var(--bg-elevated);
		border: 1px solid var(--border);
		border-radius: var(--r-lg);
		overflow: hidden;
	}
	.group-h {
		padding: var(--sp-2) var(--sp-4);
		background: var(--bg);
		border-bottom: 1px solid var(--border);
		letter-spacing: 0.06em;
		text-transform: uppercase;
	}
	.rows {
		display: flex;
		flex-direction: column;
	}
</style>
