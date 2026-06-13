<script lang="ts">
	// Shared pending-attachment chip list (CCT-236) used by the spawn modal and
	// the mid-chat composer. Renders one chip per file with a remove button and,
	// when present, the cap error.
	import { fmtSize, fileCapError } from '$lib/attachments';
	import IconButton from '$lib/components/molecules/IconButton.svelte';
	import Text from '$lib/components/atoms/Text.svelte';

	let {
		files,
		onremove,
		compact = false
	}: { files: File[]; onremove: (name: string) => void; compact?: boolean } = $props();

	const error = $derived(fileCapError(files));
</script>

{#if files.length}
	<ul class="files" class:compact>
		{#each files as f (f.name)}
			<li>
				<Text variant="code" truncate class="grow">{f.name}</Text>
				<Text size="xs" tone="faint">{fmtSize(f.size)}</Text>
				<IconButton inline class="hover-danger" icon="x" label="Remove" title="Remove" onclick={() => onremove(f.name)} />
			</li>
		{/each}
	</ul>
{/if}
{#if error}<Text size="xs" class="err">{error}</Text>{/if}

<style>
	.files {
		list-style: none;
		margin: var(--sp-1) 0 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	.files.compact {
		flex-direction: row;
		flex-wrap: wrap;
		gap: var(--sp-1) var(--sp-2);
	}
	.files li {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	.files.compact li {
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		padding: 2px var(--sp-2);
		max-width: 100%;
	}
	/* The filename and the cap error are rendered by the Text atom, so their
	   residual layout/colour chrome must be :global to reach those elements;
	   ellipsis on the filename is handled by Text's `truncate` prop. */
	:global(.grow) {
		flex: 1;
		min-width: 0;
	}
	:global(.err) {
		color: var(--c-red);
	}
</style>
