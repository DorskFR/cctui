<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import SessionCard from '$lib/components/organisms/SessionCard.svelte';
	import { draftPreview } from './sessions.logic';
	import type { SessionsPage } from './sessionsPage.svelte';

	// Drafts render through the SAME SessionCard path as every other section, so
	// they honor the card-view / compact toggles identically; the card surfaces
	// Launch/Edit/Discard in place of the live-session affordances.
	let { sp, rows }: { sp: SessionsPage; rows: SessionListItem[] } = $props();
</script>

{#if sp.cardView}
	<div class="card-grid">{@render draftItems(rows, true)}</div>
{:else}
	{@render draftItems(rows, false)}
{/if}

{#snippet draftItems(rows: SessionListItem[], grid: boolean)}
	{#each rows as s (s.id)}
		<div class="parent-row">
			<SessionCard
				session={s}
				variant={grid ? 'card' : 'row'}
				showMachine={sp.showMachine}
				accentHue={sp.accentOf(s)}
				draft
				draftLaunching={sp.launchingDraft === s.id}
				preview={draftPreview(s)}
				onLaunch={sp.launchDraft}
				onEdit={sp.editDraft}
				onDiscard={sp.discardDraft}
				onopen={() => {}}
			/>
		</div>
	{/each}
{/snippet}

<style>
	.parent-row {
		position: relative;
	}
	.card-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(min(100%, 20rem), 26.75rem));
		justify-content: start;
		align-items: stretch;
		gap: var(--sp-3);
	}
	@container (max-width: 40rem) {
		.card-grid {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
