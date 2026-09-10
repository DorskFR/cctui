<script lang="ts">
	import type { Bookmark } from '@bindings/Bookmark';
	import { goto } from '$app/navigation';
	import { ConfirmModal, Input, Spinner, Text } from '@dorsk/tsumikit';
	import PageHead from '$lib/components/molecules/PageHead.svelte';
	import BookmarkCard from '$lib/components/organisms/bookmarks/BookmarkCard.svelte';
	import BookmarkSaveModal from '$lib/components/organisms/bookmarks/BookmarkSaveModal.svelte';
	import { bookmarkMarkdown, queryTerms, sourceHref } from '$lib/bookmarks';
	import { copyText } from '$lib/clipboard';
	import { errMessage } from '$lib/api';
	import { useBookmarkActions, useBookmarks } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';

	let query = $state('');
	// Debounced so each keystroke doesn't fire a request; `q` filters server-side.
	let debounced = $state('');
	$effect(() => {
		const q = query;
		const t = setTimeout(() => (debounced = q), 200);
		return () => clearTimeout(t);
	});

	const bookmarks = useBookmarks(() => debounced);
	const actions = useBookmarkActions();
	const terms = $derived(queryTerms(debounced));
	const rows = $derived<Bookmark[]>(bookmarks.data ?? []);

	let editing = $state<Bookmark | null>(null);
	let deleting = $state<Bookmark | null>(null);

	async function open(b: Bookmark) {
		const href = sourceHref(b);
		if (href) await goto(href);
	}

	async function copy(b: Bookmark) {
		await copyText(bookmarkMarkdown(b), m.bookmarks_copy());
	}

	async function saveEdit(title: string, note: string | null) {
		const b = editing;
		editing = null;
		if (!b) return;
		try {
			await actions.update(b.id, { title, note });
			toasts.ok(m.bookmarks_updated());
		} catch (e) {
			toasts.error(m.bookmarks_save_failed({ message: errMessage(e) }));
		}
	}

	async function confirmDelete() {
		const b = deleting;
		deleting = null;
		if (!b) return;
		try {
			await actions.remove(b.id);
			toasts.ok(m.bookmarks_deleted());
		} catch (e) {
			toasts.error(errMessage(e));
		}
	}
</script>

<div class="page">
	<PageHead title={m.bookmarks_title()} />
	<Text size="sm" tone="faint">{m.bookmarks_subtitle()}</Text>

	<Input
		icon="search"
		type="search"
		aria-label={m.bookmarks_search_label()}
		placeholder={m.bookmarks_search_placeholder()}
		bind:value={query}
	/>

	{#if bookmarks.isLoading}
		<div class="msg"><Spinner /></div>
	{:else if rows.length === 0}
		<div class="msg">
			<Text size="sm" tone="faint">
				{debounced.trim() === '' ? m.bookmarks_empty() : m.bookmarks_empty_search()}
			</Text>
		</div>
	{:else}
		<div class="cards">
			{#each rows as b (b.id)}
				<BookmarkCard
					bookmark={b}
					{terms}
					onopen={open}
					oncopy={copy}
					onedit={(x) => (editing = x)}
					ondelete={(x) => (deleting = x)}
				/>
			{/each}
		</div>
	{/if}
</div>

{#if editing}
	<BookmarkSaveModal
		heading={m.bookmarks_edit_title()}
		saveLabel={m.bookmarks_save_action()}
		title={editing.title}
		note={editing.note ?? ''}
		onsave={saveEdit}
		onclose={() => (editing = null)}
	/>
{/if}

<ConfirmModal
	open={deleting !== null}
	tone="warn"
	title={m.bookmarks_delete()}
	message={m.bookmarks_delete_confirm()}
	confirmLabel={m.bookmarks_delete()}
	onconfirm={confirmDelete}
	oncancel={() => (deleting = null)}
/>

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		padding: var(--sp-4);
		max-width: 60rem;
		width: 100%;
		margin: 0 auto;
	}
	.cards {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.msg {
		display: grid;
		place-items: center;
		padding: var(--sp-6);
	}
</style>
