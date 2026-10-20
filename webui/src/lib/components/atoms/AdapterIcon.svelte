<script lang="ts">
	import type { AdapterId } from '@bindings/AdapterId';
	import BrandLogo from '$lib/components/atoms/BrandLogo.svelte';
	import { brandMark, familyAccent } from '$lib/harnesses';
	import { harnessTable } from '$lib/harnesses.svelte';

	// Brand logo wrapped in a span tinted by the harness family (Anthropic =
	// amber, OpenAI = blue, Fireworks = violet, unknown = neutral). Shared by the
	// session card, the chat header, and the accounts grid (which passes
	// `provider`).
	let {
		adapter,
		provider,
		size = 16
	}: {
		adapter?: AdapterId | null;
		provider?: string | null;
		size?: number;
	} = $props();

	const adapterId = $derived(adapter == null ? null : String(adapter));
	const mark = $derived(brandMark({ adapter: adapterId, provider }, harnessTable()));
</script>

<span
	class="adapter"
	data-mark={mark}
	style="color: {familyAccent(mark)}"
	title={provider ?? (adapterId || 'unknown')}
>
	<BrandLogo adapter={adapterId} {provider} {size} />
</span>

<style>
	.adapter {
		display: inline-flex;
		align-items: center;
		flex: none;
	}
</style>
