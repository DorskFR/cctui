<script lang="ts">
	// Shared pending-attachment chip list used by the spawn modal and
	// the mid-chat composer. Renders one chip per file with a remove button and,
	// when present, the cap error.
	import { fmtSize, fileCapError } from '$lib/attachments';
	import { IconButton, Text } from '@dorsk/tsumikit';
	import Error from '$lib/components/atoms/Error.svelte';
	import { m } from '$lib/paraglide/messages';

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
				<Text variant="code" truncate class="fname">{f.name}</Text>
				<Text size="xs" tone="faint">{fmtSize(f.size)}</Text>
				<IconButton inline class="hover-danger" icon="x"  label={m.common_remove()} title={m.common_remove()} onclick={() => onremove(f.name)} />
			</li>
		{/each}
	</ul>
{/if}
{#if error}<Error>{error}</Error>{/if}

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
	/* The filename is rendered by the Text atom, so its residual layout chrome
	   must be :global to reach that element; ellipsis is handled by Text's
	   `truncate` prop. (The cap error is now the Error atom — no reach-in.) */
	.files :global(.fname) {
		flex: 1;
		min-width: 0;
	}
</style>
