<script lang="ts">
	import type { AdapterId } from '@bindings/AdapterId';
	import BrandLogo from '$lib/components/atoms/BrandLogo.svelte';

	// Brand logo wrapped in the adapter-tinted span (Anthropic = amber,
	// Codex = blue). Shared by the session card, the chat header, and the
	// accounts grid (which passes `provider`: anthropic|openai).
	let {
		adapter,
		provider,
		size = 16
	}: {
		adapter?: AdapterId | null;
		provider?: string | null;
		size?: number;
	} = $props();

	const isCodex = $derived(
		provider != null
			? provider === 'openai'
			: (adapter ?? 'claude-code').toString().startsWith('codex')
	);
	const isFireworks = $derived(
		provider != null
			? provider === 'fireworks'
			: (adapter ?? '').toString().startsWith('opencode')
	);
</script>

<span
	class="adapter"
	class:codex={isCodex}
	class:fireworks={isFireworks}
	title={provider ?? String(adapter ?? 'claude-code')}
>
	<BrandLogo {adapter} {provider} {size} />
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
	.adapter.fireworks {
		color: var(--c-violet, var(--c-blue));
	}
</style>
