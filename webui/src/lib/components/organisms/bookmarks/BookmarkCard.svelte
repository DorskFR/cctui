<script lang="ts">
	import type { Bookmark } from '@bindings/Bookmark';
	import { Badge, Button, IconButton, Text, Timestamp, Tooltip } from '@dorsk/tsumikit';
	import { highlightBlock, renderMarkdown } from '$lib/markdown';
	import { highlightTerms } from '$lib/search';
	import { bookmarkLine, isDeadLink, sourceHref } from '$lib/bookmarks';
	import { settings } from '$lib/settings.svelte';
	import MessageBubble, {
		roleColor
	} from '$lib/components/organisms/conversation/MessageBubble.svelte';
	import { isMachineUuid } from '$lib/components/organisms/conversation/lineRender.svelte';
	import { m } from '$lib/paraglide/messages';

	let {
		bookmark,
		terms = [],
		machineId = null,
		onopen,
		oncopy,
		onedit,
		ondelete
	}: {
		bookmark: Bookmark;
		terms?: string[];
		/** The source session's machine, when it is still listed: local file
		 * paths in the body link to it the way they do in the drawer. */
		machineId?: string | null;
		onopen: (b: Bookmark) => void;
		oncopy: (b: Bookmark) => void;
		onedit: (b: Bookmark) => void;
		ondelete: (b: Bookmark) => void;
	} = $props();

	let expanded = $state(false);
	let overflows = $state(false);
	let clip = $state<HTMLDivElement | null>(null);

	const hl = (html: string) => (terms.length ? highlightTerms(html, terms) : html);
	const titleHtml = $derived(hl(escapeHtml(bookmark.title)));
	const noteHtml = $derived(bookmark.note ? hl(escapeHtml(bookmark.note)) : null);
	const line = $derived(bookmarkLine(bookmark));
	const code = $derived(line.role === 'tool' || line.role === 'result');
	const html = $derived(
		code
			? undefined
			: hl(
					renderMarkdown(line.text ?? '', {
						tables: true,
						sessionId: bookmark.session_id ?? undefined,
						machineId: machineId && isMachineUuid(machineId) ? machineId : undefined
					})
				)
	);
	const htmlCode = $derived(code ? hl(highlightBlock(line.text ?? '', line.lang ?? '')) : undefined);
	const dead = $derived(isDeadLink(bookmark));
	const href = $derived(sourceHref(bookmark));

	function escapeHtml(s: string): string {
		return s
			.replace(/&/g, '&amp;')
			.replace(/</g, '&lt;')
			.replace(/>/g, '&gt;')
			.replace(/"/g, '&quot;');
	}

	// Expand is offered only when the collapsed body actually hides something.
	$effect(() => {
		void html;
		void htmlCode;
		const el = clip;
		if (!el || expanded) return;
		const measure = () => (overflows = el.scrollHeight > el.clientHeight + 1);
		measure();
		if (typeof ResizeObserver === 'undefined') return;
		const ro = new ResizeObserver(measure);
		ro.observe(el);
		if (el.firstElementChild) ro.observe(el.firstElementChild);
		return () => ro.disconnect();
	});
</script>

<article class="card" data-role={bookmark.role}>
	<header>
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

	<div class="lmeta" style:--bc={roleColor(line.role, line.mcp)}>
		<Badge size="xs" uppercase color="var(--bc)">{line.mcp ? 'mcp' : line.role}</Badge>
		{#if code}
			<span class="tool-name">{line.role === 'result' ? '↳ ' : ''}{line.tool ?? 'tool'}</span>
		{/if}
		<Timestamp value={bookmark.message_ts} mode="datetime" tone="faint" size="xs" />
	</div>

	<div class="clip" class:collapsed={!expanded} class:faded={!expanded && overflows} bind:this={clip}>
		<MessageBubble
			role={line.role}
			mcp={line.mcp}
			{html}
			{htmlCode}
			tinted={settings.roleTintedBackground}
		/>
	</div>

	<div class="meta">
		<Text size="xs" tone="faint">
			{bookmark.session_name
				? m.bookmarks_from_session({ name: bookmark.session_name })
				: m.bookmarks_from_unknown()}
			· <Timestamp value={bookmark.created_at} mode="relative" tone="inherit" />
		</Text>
		{#if dead}
			<Badge tone="neutral">{m.bookmarks_source_deleted()}</Badge>
		{/if}
	</div>

	{#if noteHtml}
		<p class="note">{@html noteHtml}</p>
	{/if}

	<footer>
		{#if overflows || expanded}
			<Button size="sm" variant="ghost" onclick={() => (expanded = !expanded)}>
				{expanded ? m.bookmarks_collapse() : m.bookmarks_expand()}
			</Button>
		{/if}
		{#if dead}
			<Tooltip text={m.bookmarks_open_session_dead()} inline>
				{#snippet trigger()}
					<Button size="sm" disabled>{m.bookmarks_open_session()}</Button>
				{/snippet}
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
	.lmeta {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		font-size: var(--fs-xs);
		color: var(--text-faint);
	}
	.tool-name {
		font-family: var(--font-mono);
		color: var(--text-muted);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		max-width: 60%;
	}
	/* Collapsed by height, not lines: a line clamp cannot cut code blocks or
	   tables cleanly. */
	.clip.collapsed {
		max-height: 16rem;
		overflow: hidden;
	}
	.clip.faded {
		mask-image: linear-gradient(to bottom, #000 75%, transparent);
	}
	footer {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
</style>
