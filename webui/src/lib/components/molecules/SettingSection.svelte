<script lang="ts">
	// One subsection of a Settings area: glyph + title + one-line description,
	// then its groups. `id` is the anchor other screens link to.
	import type { Snippet } from 'svelte';
	import { Heading, Text } from '@dorsk/tsumikit';

	let {
		id,
		icon,
		title,
		description,
		children,
		descriptionSlot
	}: {
		id: string;
		icon: string;
		title: string;
		description?: string;
		children?: Snippet;
		descriptionSlot?: Snippet;
	} = $props();
</script>

<section {id} class="sec" data-setting-section>
	<div class="sec-h">
		<Text tone="accent" as="span">{icon}</Text>
		<Heading level={2} size="md">{title}</Heading>
	</div>
	{#if descriptionSlot}
		<Text size="sm" tone="faint" as="p" class="sec-d">{@render descriptionSlot()}</Text>
	{:else if description}
		<Text size="sm" tone="faint" as="p">{description}</Text>
	{/if}
	<div class="groups">{@render children?.()}</div>
</section>

<style>
	.sec {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		scroll-margin-top: calc(var(--header-h) + var(--sp-8));
	}
	.sec-h {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
	}
	.groups {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		margin-top: var(--sp-1);
	}
</style>
