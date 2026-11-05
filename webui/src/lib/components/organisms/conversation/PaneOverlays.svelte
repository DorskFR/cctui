<script lang="ts">
	import ForkModal from './ForkModal.svelte';
	import ForkSelectBar from './ForkSelectBar.svelte';
	import BookmarkSaveModal from '../bookmarks/BookmarkSaveModal.svelte';
	import type { ForkController } from './fork.svelte';
	import type { ForkSelection } from './forkSelect.svelte';
	import type { BookmarkSaver } from './bookmarkSave.svelte';
	import { m } from '$lib/paraglide/messages';

	let {
		fork,
		forkSelect,
		bookmarks,
		archived,
		isCodexSession,
		contained,
		onforkselection
	}: {
		fork: ForkController;
		forkSelect: ForkSelection;
		bookmarks: BookmarkSaver;
		archived: boolean;
		isCodexSession: boolean;
		/** Inside a tile: the select bar sits in the pane, not on the viewport. */
		contained: boolean;
		onforkselection: () => void;
	} = $props();
</script>

{#if forkSelect.active}
	<ForkSelectBar
		{contained}
		count={forkSelect.selected.size}
		onfork={onforkselection}
		onforkall={fork.openDialog}
		oncancel={forkSelect.exit}
	/>
{/if}

{#if fork.open}
	<ForkModal
		{archived}
		{isCodexSession}
		parentTokens={fork.parentTokens}
		models={fork.models}
		efforts={fork.efforts}
		forking={fork.forking}
		extractLabel={fork.extractLabel}
		bind:model={fork.model}
		bind:effort={fork.effort}
		bind:prompt={fork.prompt}
		oncancel={fork.cancel}
		onsubmit={fork.submit}
	/>
{/if}

{#if bookmarks.draft}
	<BookmarkSaveModal
		heading={m.bookmarks_save_title()}
		saveLabel={m.bookmarks_save_action()}
		title={bookmarks.draft.title}
		onsave={bookmarks.save}
		onclose={bookmarks.close}
	/>
{/if}
