<script lang="ts">
	import type { Bookmark } from '@bindings/Bookmark';
	import { Badge, Button, IconButton, Text, Tooltip } from '@dorsk/tsumikit';
	import { relativeTime } from '$lib/format';
	import { renderMarkdown } from '$lib/markdown';
	import { highlightTerms } from '$lib/search';
	import { isDeadLink, sourceHref } from '$lib/bookmarks';
	import { m } from '$lib/paraglide/messages';

	let {
		bookmark,
		terms = [],
		onopen,
		oncopy,
		onedit,
		ondelete
	}: {
		bookmark: Bookmark;
		terms?: string[];
		onopen: (b: Bookmark) => void;
		oncopy: (b: Bookmark) => void;
		onedit: (b: Bookmark) => void;
		ondelete: (b: Bookmark) => void;
	} = $props();

	let expanded = $state(false);

	const hl = (html: string) => (terms.length ? highlightTerms(html, terms) : html);
	const titleHtml = $derived(hl(escapeHtml(bookmark.title)));
	const noteHtml = $derived(bookmark.note ? hl(escapeHtml(bookmark.note)) : null);
	const bodyHtml = $derived(hl(renderMarkdown(bookmark.body, { tables: true })));
	const dead = $derived(isDeadLink(bookmark));
	const href = $derived(sourceHref(bookmark));

	function escapeHtml(s: string): string {
		return s
			.replace(/&/g, '&amp;')
			.replace(/</g, '&lt;')
			.replace(/>/g, '&gt;')
			.replace(/"/g, '&quot;');
	}
</script>

<article class="card" data-role={bookmark.role}>
	<header>
		<span class="dot" aria-hidden="true"></span>
		<h3 class="title">{@html titleHtml}</h3>
		<span class="actions">
			<IconButton
				icon="markdown"
				label={m.bookmarks_copy()}
				title={m.bookmarks_copy()}
				onclick={() => oncopy(bookmark)}
			/>
			<IconButton
				icon="edit"
				label={m.bookmarks_edit()}
				title={m.bookmarks_edit()}
				onclick={() => onedit(bookmark)}
			/>
			<IconButton
				icon="trash"
				label={m.bookmarks_delete()}
				title={m.bookmarks_delete()}
				onclick={() => ondelete(bookmark)}
			/>
		</span>
	</header>

	<div class="meta">
		<Text size="xs" tone="faint">
			{bookmark.session_name
				? m.bookmarks_from_session({ name: bookmark.session_name })
				: m.bookmarks_from_unknown()}
			· {relativeTime(bookmark.created_at)}
		</Text>
		{#if dead}
			<Badge tone="neutral">{m.bookmarks_source_deleted()}</Badge>
		{/if}
	</div>

	{#if noteHtml}
		<p class="note">{@html noteHtml}</p>
	{/if}

	<div class="body md" class:clamped={!expanded}>{@html bodyHtml}</div>

	<footer>
		<Button size="sm" variant="ghost" onclick={() => (expanded = !expanded)}>
			{expanded ? m.bookmarks_collapse() : m.bookmarks_expand()}
		</Button>
		{#if dead}
			<Tooltip text={m.bookmarks_open_session_dead()}>
				<Button size="sm" disabled>{m.bookmarks_open_session()}</Button>
			</Tooltip>
		{:else}
			<Button size="sm" onclick={() => onopen(bookmark)} title={href ?? undefined}>
				{m.bookmarks_open_session()}
			</Button>
		{/if}
	</footer>
</article>

<style>
	.card {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-3);
		border: 1px solid var(--border);
		border-radius: var(--radius-md);
		background: var(--surface);
	}
	header {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.dot {
		width: 0.5rem;
		height: 0.5rem;
		border-radius: 50%;
		background: var(--role-assistant);
		flex: none;
	}
	.card[data-role='user'] .dot {
		background: var(--role-user);
	}
	.card[data-role='system'] .dot {
		background: var(--role-system, var(--text-faint));
	}
	.card[data-role='tool'] .dot,
	.card[data-role='result'] .dot {
		background: var(--role-tool);
	}
	.title {
		margin: 0;
		font-size: var(--fs-md);
		font-weight: var(--fw-medium);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.actions {
		margin-left: auto;
		display: inline-flex;
		gap: var(--sp-1);
	}
	.meta {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.note {
		margin: 0;
		padding-left: var(--sp-2);
		border-left: 2px solid var(--border);
		color: var(--text-muted);
		font-size: var(--fs-sm);
	}
	.body {
		overflow-wrap: anywhere;
	}
	/* Re-reading the body is the feature, so it is only lightly collapsed. */
	.body.clamped {
		display: -webkit-box;
		-webkit-line-clamp: 6;
		line-clamp: 6;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}
	footer {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
</style>
