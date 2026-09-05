<script lang="ts">
	// A file as a small bordered box: glyph · name (truncating) · size. NOT
	// tsumikit's `Button chip`, which is a fixed 2.5rem square meant for a lone
	// glyph — file names spill straight out of it.
	import { Icon, Text } from '@dorsk/tsumikit';
	import { fmtSize } from '$lib/attachments';

	let {
		name,
		size = null,
		detail = null,
		title,
		expanded = null,
		unavailable = false,
		onclick
	}: {
		name: string;
		/** Bytes; omitted when `detail` says something better (e.g. a line count). */
		size?: number | null;
		detail?: string | null;
		title?: string;
		/** Non-null renders a disclosure caret in that state. */
		expanded?: boolean | null;
		/** The session was archived: the blob is gone, so show it inert. */
		unavailable?: boolean;
		onclick?: () => void;
	} = $props();
</script>

<button
	type="button"
	class="file-chip"
	class:unavailable
	{title}
	disabled={unavailable}
	aria-expanded={expanded === null ? undefined : expanded}
	onclick={() => onclick?.()}
>
	<span class="glyph"><Icon name={expanded === null ? 'file-text' : expanded ? 'chevron-down' : 'chevron-right'} size={12} /></span>
	<span class="name">{name}</span>
	{#if detail ?? size !== null}
		<Text size="xs" tone="faint" as="span">{detail ?? fmtSize(size ?? 0)}</Text>
	{/if}
</button>

<style>
	.file-chip {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
		max-width: 100%;
		padding: 3px var(--sp-2);
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		background: var(--bg-elevated);
		color: var(--text);
		cursor: pointer;
		text-align: left;
	}
	.file-chip:hover:not(.unavailable) {
		background: var(--bg-elevated-2);
		border-color: var(--border-strong);
	}
	.file-chip.unavailable {
		opacity: 0.5;
		cursor: default;
	}
	.glyph {
		display: inline-flex;
		flex: none;
		color: var(--text-faint);
	}
	.name {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-family: var(--font-mono);
		font-size: var(--fs-xs);
	}
</style>
