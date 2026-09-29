<script lang="ts">
	// Find-in-conversation bar: the transcript's own FilterSearchBar plus the
	// hit stepper that used to live in DrawerToolbar. The counter reads the
	// server's list, so it is right even for matches older than the loaded page.
	import { Field, FilterSearchBar, IconButton, Toggle } from '@dorsk/tsumikit';
	import type { ConversationSearch } from './convSearch.svelte';
	import { buildConversationSearchSchema, conversationSearchPlaceholder } from './searchSchema';
	import { m } from '$lib/paraglide/messages';

	let { search }: { search: ConversationSearch } = $props();

	const schema = buildConversationSearchSchema(() => search.tools);
	const searchId = $props.id();

	let inputHost = $state<HTMLElement | null>(null);
	$effect(() => {
		inputHost?.querySelector('input')?.focus();
	});

	const counter = $derived(
		search.count === 0
			? m.conversation_search_no_hits()
			: m.conversation_hit_counter({
					n: search.index + 1,
					total: search.truncated ? `${search.count}+` : search.count
				})
	);

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') {
			e.preventDefault();
			void (e.shiftKey ? search.prev() : search.next());
			return;
		}
		if (e.key === 'Escape' && search.escape()) {
			e.preventDefault();
			e.stopPropagation();
		}
	}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="convsearch" data-journey="conversation-search" {onkeydown}>
	<div class="field" bind:this={inputHost}>
		<label for={searchId} class="sr-only">{m.conversation_search_label()}</label>
		<Field for={searchId}>
			<FilterSearchBar
				{schema}
				size="sm"
				showChips
				grow
				value={search.rawQuery}
				placeholder={conversationSearchPlaceholder()}
				onchange={(_q, raw) => search.setQuery(raw)}
			/>
		</Field>
	</div>
	<div class="hitbar" role="group" aria-label={m.conversation_hits_aria()}>
		<Toggle
			pressed={false}
			disabled={search.count === 0}
			title={m.conversation_hit_prev()}
			onclick={() => void search.prev()}><span class="glyph">↑</span></Toggle
		>
		<Toggle
			pressed={false}
			data-journey="hit-next"
			disabled={search.count === 0}
			title={m.conversation_hit_next()}
			onclick={() => void search.next()}><span class="glyph">↓</span></Toggle
		>
		<span class="hit-count" aria-live="polite">{counter}</span>
		<IconButton
			icon="x"
			data-journey="find-close"
			box="sm"
			label={m.conversation_search_close()}
			onclick={search.close}
		/>
	</div>
</div>

<style>
	.convsearch {
		display: flex;
		align-items: flex-start;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
		border-bottom: 1px solid var(--border);
		background: var(--bg-elevated);
	}
	.field {
		flex: 1 1 auto;
		min-width: 0;
	}
	.hitbar {
		display: flex;
		flex: none;
		align-items: center;
		gap: var(--sp-1);
	}
	.glyph {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		min-width: 1em;
		line-height: 1;
	}
	.hit-count {
		color: var(--text-muted);
		font-variant-numeric: tabular-nums;
		white-space: nowrap;
		font-size: var(--fs-xs);
	}
	/* Below the drawer's mobile breakpoint the bar takes the full width and the
	   stepper drops to its own row, keeping ↑/↓ under a thumb. */
	@media (max-width: 959px) {
		.convsearch {
			flex-wrap: wrap;
		}
		.field {
			flex: 1 0 100%;
		}
		.hitbar {
			width: 100%;
			justify-content: flex-end;
		}
	}
</style>
