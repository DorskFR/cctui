<script lang="ts">
	// Excluded from the saved image.
	import { Icon, IconButton, Menu, type MenuItem } from '@dorsk/tsumikit';
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
		onquote,
		onspeak,
		speaking = false
	}: {
		ln: Line;
		pinnable: boolean;
		pinned?: boolean;
		onpin?: (ln: Line) => void;
		onsaveimage: (node: HTMLElement, ln: Line) => void;
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

	const items = $derived.by(() => {
		const out: MenuItem[] = [];
		if (pinnable) {
			out.push({
				id: 'pin',
				label: pinned ? m.conversation_unpin_label() : m.conversation_pin_label(),
				icon: pinned ? 'pin-off' : 'pin',
				onselect: () => onpin?.(ln)
			});
		}
		if (onspeak) {
			out.push({
				id: 'speak',
				label: speaking ? m.voice_line_speaking() : m.voice_line_speak(),
				icon: speaking ? 'stop' : 'play',
				onselect: () => onspeak?.(ln)
			});
		}
		out.push({
			id: 'markdown',
			label: m.conversation_copy_markdown_label(),
			icon: 'markdown',
			onselect: () => oncopymarkdown(ln)
		});
		out.push({
			id: 'image',
			label: m.conversation_save_image_label(),
			icon: 'image',
			onselect: () => {
				const line = root?.closest<HTMLElement>('.line');
				if (line) onsaveimage(line, ln);
			}
		});
		if (onbookmark) {
			out.push({
				id: 'bookmark',
				label: m.bookmarks_line_label(),
				icon: 'bookmark',
				pressed: bookmarked,
				attrs: {
					title: bookmarked ? m.bookmarks_line_saved_title() : m.bookmarks_line_title()
				},
				onselect: () => onbookmark?.(ln)
			});
		}
		return out;
	});
</script>

<span class="line-actions" class:has-pin={pinned} bind:this={root} data-journey="line-actions">
	{#if speaking}
		<IconButton
			inline
			glyphSize={16}
			icon="stop"
			pressed
			style="--btn-on: var(--role-assistant)"
			label={m.voice_line_speaking()}
			title={m.voice_line_speaking()}
			onclick={() => onspeak?.(ln)}
		/>
	{/if}
	{#if onquote}
		<IconButton
			inline
			glyphSize={16}
			icon="quote"
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
	<Menu label={m.tx_line_more()} {items} placement="bottom-end" variant="ghost" box="sm">
		{#snippet trigger()}<Icon name="more" size={16} />{/snippet}
	</Menu>
</span>

<style>
	.line-actions {
		margin-left: auto;
		display: inline-flex;
		flex: none;
		align-items: center;
		gap: var(--sp-1);
	}
</style>
