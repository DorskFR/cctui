<script lang="ts">
	import { Icon, Popover } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { promptHistory } from '$lib/drafts';

	// The discoverable half of spawn prompt recall: ArrowUp in the textarea is
	// invisible, so both spawn branches also get this trigger + list of recent
	// prompts. Picking one goes through the caller's HistoryNav so keyboard and
	// mouse recall share one cursor.
	let { onpick, disabled = false }: { onpick: (value: string) => void; disabled?: boolean } = $props();

	let entries = $state<string[]>([]);

	function preview(value: string) {
		const text = value.trim();
		return text.length > 400 ? `${text.slice(0, 400)}…` : text;
	}
</script>

<Popover
	label={m.spawn_prompt_history()}
	placement="bottom-start"
	role="menu"
	haspopup="menu"
	box="sm"
	hitArea="compact"
	{disabled}
	panelStyle="width:min(26rem,calc(100vw - 5rem));max-height:18rem;overflow-y:auto"
	onopen={() => (entries = promptHistory.get().slice().reverse())}
>
	{#snippet trigger()}
		<Icon name="clock" size={16} />
	{/snippet}
	{#snippet children({ close })}
		{#if entries.length === 0}
			<p class="empty">{m.spawn_prompt_history_empty()}</p>
		{:else}
			{#each entries as entry, i (`${i}:${entry}`)}
				<button
					type="button"
					role="menuitem"
					class="entry"
					title={entry}
					onclick={() => {
						onpick(entry);
						close();
					}}
				>
					{preview(entry)}
				</button>
			{/each}
			<button
				type="button"
				class="clear"
				onclick={() => {
					promptHistory.clear();
					entries = [];
				}}
			>
				{m.spawn_prompt_history_clear()}
			</button>
		{/if}
	{/snippet}
</Popover>

<style>
	.empty {
		margin: 0;
		padding: var(--sp-2);
		color: var(--text-faint);
		font-size: var(--fs-xs);
	}
	.entry {
		display: -webkit-box;
		-webkit-box-orient: vertical;
		-webkit-line-clamp: 3;
		line-clamp: 3;
		width: 100%;
		padding: var(--sp-1) var(--sp-2);
		border: none;
		border-radius: var(--r-sm);
		background: none;
		color: var(--text);
		font-family: var(--font-mono);
		font-size: var(--fs-xs);
		text-align: left;
		white-space: pre-wrap;
		overflow: hidden;
		overflow-wrap: anywhere;
		cursor: pointer;
	}
	.entry + .entry {
		border-top: 1px solid var(--border);
	}
	.entry:hover {
		background: var(--bg-elevated-3, var(--bg-elevated-2));
	}
	.clear {
		display: block;
		width: 100%;
		margin-top: var(--sp-1);
		padding: var(--sp-1) var(--sp-2);
		border: none;
		border-top: 1px solid var(--border);
		background: none;
		color: var(--text-faint);
		font-size: var(--fs-xs);
		text-align: left;
		cursor: pointer;
	}
	.clear:hover {
		color: var(--text);
	}
</style>
