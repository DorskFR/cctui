<script lang="ts">
	import { Icon, IconButton, Menu, type MenuItem } from '@dorsk/tsumikit';
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

	const items: MenuItem[] = $derived([
		...(pinnable
			? [
					{
						label: pinned ? m.conversation_unpin_label() : m.conversation_pin_label(),
						icon: 'pin' as const,
						pressed: pinned,
						onselect: () => onpin?.(ln)
					}
				]
			: []),
		...(onspeak && !speaking
			? [{ label: m.voice_line_speak(), icon: 'play' as const, onselect: () => onspeak?.(ln) }]
			: []),
		{
			label: m.conversation_copy_markdown_label(),
			icon: 'markdown' as const,
			onselect: () => oncopymarkdown(ln)
		},
		{
			label: m.conversation_save_image_label(),
			icon: 'image' as const,
			onselect: () => root && onsaveimage(root, ln)
		},
		...(onbookmark
			? [
					{
						label: m.bookmarks_line_label(),
						icon: 'bookmark' as const,
						pressed: bookmarked,
						onselect: () => onbookmark?.(ln)
					}
				]
			: [])
	]);

	// Clicking collapses the selection, so it is read on pointerdown.
	let picked: string | null = null;
	function grabSelection(e: Event) {
		const line = (e.currentTarget as HTMLElement).closest('.line');
		picked = bubbleSelection(line?.querySelector('.bubble') ?? null);
	}
</script>

<span class="line-actions" bind:this={root} data-journey="line-actions">
	{#if onspeak && speaking}
		<IconButton
			inline
			glyphSize={16}
			icon="stop"
			pressed
			style="--accent: var(--role-assistant)"
			label={m.voice_line_speaking()}
			title={m.voice_line_speaking()}
			onclick={() => onspeak?.(ln)}
		/>
	{/if}
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
	<Menu label={m.conversation_line_actions()} {items} placement="bottom-end" box="sm" title={m.conversation_line_actions()}>
		{#snippet trigger()}<Icon name="more" size={16} />{/snippet}
	</Menu>
</span>

<style>
	.line-actions {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
	}
</style>
