<script lang="ts">
	// Per-message action cluster at the right of the meta row: pin, save-as-image,
	// copy-as-Markdown, quote-reply, bookmark. Excluded from the saved image.
	import { Icon, IconButton } from '@dorsk/tsumikit';
	import { bubbleSelection } from './lineActions';
	import type { Line } from './types';
	import { m } from '$lib/paraglide/messages';

	let {
		ln,
		pinnable,
		pinned = false,
		onpin,
		onsaveimage,
		oncopymarkdown,
		onbookmark,
		bookmarked = false,
		onquote
	}: {
		ln: Line;
		pinnable: boolean;
		pinned?: boolean;
		onpin?: (ln: Line) => void;
		onsaveimage: (e: MouseEvent, ln: Line) => void;
		oncopymarkdown: (ln: Line) => void;
		/** Omit to hide the bookmark action. */
		onbookmark?: (ln: Line) => void;
		bookmarked?: boolean;
		/** Quote this line (or the selection inside it) into the composer;
		 * omit to hide the action. */
		onquote?: (ln: Line, selection: string | null) => void;
	} = $props();

	// Clicking collapses the selection, so it is read on pointerdown.
	let picked: string | null = null;
	function grabSelection(e: Event) {
		const line = (e.currentTarget as HTMLElement).closest('.line');
		picked = bubbleSelection(line?.querySelector('.bubble') ?? null);
	}
</script>

<span class="line-actions" class:has-pin={pinned} data-journey="line-actions">
	{#if pinnable}
		<button
			type="button"
			class="pin-btn"
			class:on={pinned}
			aria-pressed={pinned}
			aria-label={pinned ? m.conversation_unpin_label() : m.conversation_pin_label()}
			title={pinned ? m.conversation_unpin_title() : m.conversation_pin_title()}
			onclick={() => onpin?.(ln)}><Icon name="pin" size={16} filled={pinned} /></button
		>
	{/if}
	<!-- Copy-as-Markdown uses the same markdown glyph as the
	     conversation-level copy; save-as-image uses a
	     plain image icon and sits right next to it. -->
	<IconButton
		inline
		glyphSize={16}
		icon="image"
		label={m.conversation_save_image_label()}
		title={m.conversation_save_image_title()}
		onclick={(e) => onsaveimage(e, ln)}
	/>
	<IconButton
		inline
		glyphSize={16}
		icon="markdown"
		label={m.conversation_copy_markdown_label()}
		title={m.conversation_copy_markdown_title()}
		onclick={() => oncopymarkdown(ln)}
	/>
	{#if onquote}
		<IconButton
			inline
			glyphSize={16}
			icon="back"
			class="quote-btn"
			label={m.conversation_quote_label()}
			title={m.conversation_quote_title()}
			onpointerdown={grabSelection}
			onmousedown={grabSelection}
			onclick={() => {
				onquote?.(ln, picked);
				picked = null;
			}}
		/>
	{/if}
	{#if onbookmark}
		<button
			type="button"
			class="bookmark"
			class:saved={bookmarked}
			aria-label={m.bookmarks_line_label()}
			title={bookmarked ? m.bookmarks_line_saved_title() : m.bookmarks_line_title()}
			onclick={() => onbookmark?.(ln)}>◈</button
		>
	{/if}
</span>

<style>
	.line-actions {
		margin-left: auto;
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
	}
	/* The pin stays visible once set — it marks the line in the flow, so it
	   cannot be a hover-only affordance like the copy buttons. */
	.pin-btn {
		display: inline-flex;
		align-items: center;
		padding: 0 var(--sp-1);
		background: none;
		border: none;
		line-height: 1;
		color: var(--text-faint);
		cursor: pointer;
	}
	.pin-btn:hover,
	.pin-btn.on {
		color: var(--warn);
	}
	.line-actions .bookmark {
		display: inline-flex;
		align-items: center;
		padding: var(--sp-1);
		background: none;
		border: 0;
		line-height: 1;
		cursor: pointer;
		font-size: var(--fs-sm);
		color: var(--text-muted);
	}
	.line-actions .bookmark:hover {
		color: var(--text);
	}
	.line-actions .bookmark.saved {
		color: var(--role-assistant);
	}
</style>
