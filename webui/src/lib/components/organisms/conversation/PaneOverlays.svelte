<script lang="ts">
	import { m } from '#lib/paraglide/messages.js';
	import ForkModal from './ForkModal.svelte';
	import ForkSelectBar from './ForkSelectBar.svelte';
	import BookmarkSaveModal from '../bookmarks/BookmarkSaveModal.svelte';
	import type { PaneModel } from './paneModel.svelte';

	let { p, canFork, contained }: { p: PaneModel; canFork: boolean; contained: boolean } = $props();
	const fork = $derived(p.fork);
	const forkSelect = $derived(p.forkSelect);
</script>

{#if forkSelect.active && canFork}
	<ForkSelectBar
		{contained}
		count={forkSelect.selected.size}
		onfork={p.forkSelection}
		onforkall={fork.openDialog}
		oncancel={forkSelect.exit}
	/>
{/if}

{#if fork.open && canFork}
	<ForkModal
		archived={p.archived}
		isCodexSession={p.isCodexSession}
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

{#if p.bookmarks.draft}
	<BookmarkSaveModal
		heading={m.bookmarks_save_title()}
		saveLabel={m.bookmarks_save_action()}
		title={p.bookmarks.draft.title}
		onsave={p.bookmarks.save}
		onclose={p.bookmarks.close}
	/>
{/if}
