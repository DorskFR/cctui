<script lang="ts">
	import { IconButton } from '@dorsk/tsumikit';
	import { bubbleSelection } from './lineActions';
	import type { Line } from './types';
	import { m } from '#lib/paraglide/messages.js';

	let {
		ln,
		pinnable,
		pinned = false,
		onpin,
		onsaveimage,
		oncopymarkdown,
		onbookmark,
		bookmarked = false,
		onquote,
		onspeak,
		speaking = false
	}: {
		ln: Line;
		pinnable: boolean;
		pinned?: boolean;
		onpin?: (ln: Line) => void;
		/** `from` is any node inside the line to capture. */
		onsaveimage: (from: Element, ln: Line) => void;
		oncopymarkdown: (ln: Line) => void;
		/** Omit to hide the bookmark action. */
		onbookmark?: (ln: Line) => void;
		bookmarked?: boolean;
		/** Quote this line (or the selection inside it) into the composer;
		 * omit to hide the action. */
		onquote?: (ln: Line, selection: string | null) => void;
		/** Read this line aloud; omit to hide the action. */
		onspeak?: (ln: Line) => void;
		speaking?: boolean;
	} = $props();

	let root = $state<HTMLElement>();

	// Clicking collapses the selection, so it is read on pointerdown.
	let picked: string | null = null;
	function grabSelection(e: Event) {
		const line = (e.currentTarget as HTMLElement).closest('.line');
		picked = bubbleSelection(line?.querySelector('.bubble') ?? null);
	}
</script>

<span class="line-actions" bind:this={root} data-journey="line-actions">
	{#if pinnable}
		<IconButton
			inline
			glyphSize={16}
			icon="pin"
			filled={pinned}
			pressed={pinned}
			style={pinned ? '--accent: var(--warn)' : undefined}
			label={pinned ? m.conversation_unpin_label() : m.conversation_pin_label()}
			title={pinned ? m.conversation_unpin_title() : m.conversation_pin_label()}
			onclick={() => onpin?.(ln)}
		/>
	{/if}
	<IconButton
		inline
		glyphSize={16}
		icon="image"
		label={m.conversation_save_image_label()}
		title={m.conversation_save_image_label()}
		onclick={() => root && onsaveimage(root, ln)}
	/>
	<IconButton
		inline
		glyphSize={16}
		icon="markdown"
		label={m.conversation_copy_markdown_label()}
		title={m.conversation_copy_markdown_label()}
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
	{#if onspeak}
		<IconButton
			inline
			glyphSize={16}
			icon={speaking ? 'stop' : 'play'}
			pressed={speaking}
			style={speaking ? '--accent: var(--role-assistant)' : undefined}
			label={speaking ? m.voice_line_speaking() : m.voice_line_speak()}
			title={speaking ? m.voice_line_speaking() : m.voice_line_speak()}
			onclick={() => onspeak?.(ln)}
		/>
	{/if}
	{#if onbookmark}
		<IconButton
			inline
			glyphSize={16}
			icon="bookmark"
			filled={bookmarked}
			pressed={bookmarked}
			style={bookmarked ? '--accent: var(--role-assistant)' : undefined}
			label={m.bookmarks_line_label()}
			title={bookmarked ? m.bookmarks_line_saved_title() : m.bookmarks_line_label()}
			onclick={() => onbookmark?.(ln)}
		/>
	{/if}
</span>

<style>
	.line-actions {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
	}
</style>
