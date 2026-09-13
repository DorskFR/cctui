<script lang="ts">
	import { Button, Carousel, Heading, Modal, Stack, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import type { DeckCard } from '$lib/welcomeDeck.svelte';

	let {
		cards,
		reached,
		onindex,
		onfinish,
		ondismiss
	}: {
		cards: DeckCard[];
		/** Furthest card the journey engine has reached. */
		reached: number;
		onindex: (index: number) => void;
		onfinish: () => void;
		ondismiss: () => void;
	} = $props();

	let index = $state(0);

	$effect(() => {
		if (reached > index) index = reached;
	});

	const last = $derived(index >= cards.length - 1);

	function change(next: number) {
		index = next;
		onindex(next);
	}
</script>

{#snippet card(entry: DeckCard)}
	<div class="deck-card">
		<Stack gap="sm">
			<Heading level={2} size="lg">{entry.title}</Heading>
			<Text tone="muted">{entry.body}</Text>
		</Stack>
	</div>
{/snippet}

{#snippet body()}
	<Carousel
		slides={cards}
		slide={card}
		bind:index
		onchange={change}
		label={m.guide_welcome_carousel_label()}
		dots={cards.length <= 8}
		counter
	/>
{/snippet}

{#snippet footer()}
	{#if last}
		<Button variant="primary" onclick={onfinish}>{m.guide_welcome_finish()}</Button>
	{:else}
		<Button variant="ghost" onclick={ondismiss}>{m.guide_welcome_skip()}</Button>
	{/if}
{/snippet}

<Modal title={m.guide_welcome_title()} size="lg" onclose={ondismiss} {body} {footer} />

<style>
	.deck-card {
		min-height: 11rem;
		padding-block: var(--sp-2);
	}
</style>
