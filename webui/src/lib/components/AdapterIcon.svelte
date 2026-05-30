<script lang="ts">
	import type { AdapterId } from '@bindings/AdapterId';
	import BrandLogo from './BrandLogo.svelte';

	// Brand logo wrapped in the adapter-tinted span (Anthropic = amber,
	// Codex = blue). Shared by the session card and the chat header.
	let {
		adapter,
		size = 16
	}: {
		adapter?: AdapterId | null;
		size?: number;
	} = $props();

	const isCodex = $derived((adapter ?? 'claude-code').toString().startsWith('codex'));
</script>

<span class="adapter" class:codex={isCodex} title={String(adapter ?? 'claude-code')}>
	<BrandLogo {adapter} {size} />
</span>

<style>
	.adapter {
		display: inline-flex;
		align-items: center;
		/* Anthropic = warm/orange, Codex = teal-blue, matching brand hues. */
		color: var(--c-amber);
		flex: none;
	}
	.adapter.codex {
		color: var(--c-blue);
	}
</style>
