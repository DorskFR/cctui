<script lang="ts">
	// Shared pending-attachment chip list (CCT-236) used by the spawn modal and
	// the mid-chat composer. Renders one chip per file with a remove button and,
	// when present, the cap error.
	import { fmtSize, fileCapError } from '$lib/attachments';
	import IconButton from '$lib/components/molecules/IconButton.svelte';

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
				<code class="grow">{f.name}</code>
				<span class="faint sz">{fmtSize(f.size)}</span>
				<IconButton inline class="hover-danger" icon="x" label="Remove" title="Remove" onclick={() => onremove(f.name)} />
			</li>
		{/each}
	</ul>
{/if}
{#if error}<span class="err sz">{error}</span>{/if}

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
	.grow {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.sz {
		font-size: var(--fs-xs);
	}
	.err {
		color: var(--c-red);
	}
</style>
