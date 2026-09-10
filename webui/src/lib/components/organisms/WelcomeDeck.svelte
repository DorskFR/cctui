<script lang="ts">
	import { Button, Carousel, Heading, Modal, Stack, Text } from '@dorsk/tsumikit';
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
		label={cards[0]?.title ?? 'Welcome'}
		counter
	/>
{/snippet}

{#snippet footer()}
	{#if last}
		<Button variant="primary" onclick={onfinish}>Get started</Button>
	{:else}
		<Button variant="ghost" onclick={ondismiss}>Skip</Button>
	{/if}
{/snippet}

<Modal title="Welcome to cctui" size="lg" onclose={ondismiss} {body} {footer} />

<style>
	.deck-card {
		min-height: 9rem;
		padding-block: var(--space-sm);
	}
</style>
