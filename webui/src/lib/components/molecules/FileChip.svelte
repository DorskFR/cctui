<script lang="ts">
	// A file as a chip: glyph · name (truncating) · size. Uses the kit's Badge in
	// button form — the same shape WorkingDir builds its path chip from.
	import { Badge, Text } from '@dorsk/tsumikit';
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

<span class="chip" class:unavailable>
	<Badge
		as="button"
		mono
		truncate
		maxWidth="100%"
		icon={expanded === null ? 'file-text' : expanded ? 'chevron-down' : 'chevron-right'}
		{title}
		disabled={unavailable}
		aria-expanded={expanded === null ? undefined : expanded}
		onclick={() => onclick?.()}
	>
		{name}
		{#if detail ?? size !== null}
			<Text size="xs" tone="faint" as="span">{detail ?? fmtSize(size ?? 0)}</Text>
		{/if}
	</Badge>
</span>

<style>
	.chip {
		display: inline-flex;
		min-width: 0;
		max-width: 100%;
	}
	.unavailable {
		opacity: 0.5;
	}
</style>
