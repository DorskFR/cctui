<script lang="ts">
	// One anchor target of the Settings screen: glyph + title + one-line
	// description, then its groups. `id` is what the sticky table of contents
	// links to and what the scroll spy reports.
	import type { Snippet } from 'svelte';
	import { Badge, Heading, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		id,
		icon,
		title,
		description,
		admin = false,
		children,
		descriptionSlot
	}: {
		id: string;
		icon: string;
		title: string;
		description?: string;
		admin?: boolean;
		children?: Snippet;
		descriptionSlot?: Snippet;
	} = $props();
</script>

<section {id} class="sec" data-setting-section>
	<div class="sec-h">
		<Text tone="accent" as="span">{icon}</Text>
		<Heading level={2} size="md">{title}</Heading>
		{#if admin}<Badge tone="warn" size="sm" uppercase border>{m.settings_scope_admin()}</Badge>{/if}
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
