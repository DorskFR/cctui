<script lang="ts">
	// Shared pending-attachment chip list used by the spawn modal and
	// the mid-chat composer. Renders one chip per file with a remove button and,
	// when present, the cap error.
	import { fmtSize, fileCapError } from '$lib/attachments';
	import { Button, Icon, IconButton, Popover, Text } from '@dorsk/tsumikit';
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
	<div class="wrap" class:compact>
		<ul class="files" class:compact>
			{#each files as f (f.name)}
				<li class="full">
					<Text variant="code" truncate grow class="fname">{f.name}</Text>
					<Text size="xs" tone="faint">{fmtSize(f.size)}</Text>
					<IconButton inline class="hover-danger" icon="x" label={m.common_remove()} title={m.common_remove()} onclick={() => onremove(f.name)} />
				</li>
				{#if compact}
					<li class="tile">
						<Popover label={f.name} box="sm" placement="top-start">
							{#snippet trigger()}<Icon name="file-text" size={16} />{/snippet}
							{#snippet children({ close })}
								<div class="tile-detail">
									<Text variant="code" size="sm" style="overflow-wrap:anywhere">{f.name}</Text>
									<Text size="xs" tone="faint">{fmtSize(f.size)}</Text>
									<Button
										size="sm"
										tone="danger"
										onclick={() => {
											close();
											onremove(f.name);
										}}>{m.common_remove()}</Button
									>
								</div>
							{/snippet}
						</Popover>
					</li>
				{/if}
			{/each}
		</ul>
	</div>
{/if}
{#if error}<Error>{error}</Error>{/if}

<style>
	.wrap.compact {
		container: attachments / inline-size;
	}
	.tile {
		display: none;
	}
	@container attachments (max-width: 30rem) {
		.files.compact li.full {
			display: none;
		}
		.files.compact li.tile {
			display: flex;
			padding: 0;
			border: none;
			background: none;
		}
	}
	.tile-detail {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: var(--sp-1);
		max-width: 16rem;
	}
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
</style>
