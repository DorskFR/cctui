<script lang="ts">
	import { Button, Icon, Popover } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { promptHistory } from '$lib/drafts';

	// The discoverable half of spawn prompt recall: ArrowUp in the textarea is
	// invisible, so both spawn branches also get this trigger + list of recent
	// prompts. Picking one goes through the caller's HistoryNav so keyboard and
	// mouse recall share one cursor.
	let { onpick, disabled = false }: { onpick: (value: string) => void; disabled?: boolean } = $props();

	let entries = $state<string[]>([]);

	// Rows are multi-line previews, not one-line labels; `style` is the only hook
	// that reaches the element Button renders.
	const ENTRY =
		'justify-content:flex-start;text-align:left;height:auto;min-height:0;white-space:pre-wrap;font-family:var(--font-mono);font-size:var(--fs-xs)';

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
				<Button
					variant="ghost"
					size="sm"
					block
					role="menuitem"
					style={ENTRY}
					title={entry}
					onclick={() => {
						onpick(entry);
						close();
					}}
				>
					<span class="clamp">{preview(entry)}</span>
				</Button>
			{/each}
			<!-- The footer resets the list in place, so it stays outside the
			     menuitem set and leaves the panel open. -->
			<Button
				variant="ghost"
				size="sm"
				block
				style={ENTRY}
				onclick={() => {
					promptHistory.clear();
					entries = [];
				}}
			>
				{m.spawn_prompt_history_clear()}
			</Button>
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
	.clamp {
		display: -webkit-box;
		-webkit-box-orient: vertical;
		-webkit-line-clamp: 3;
		line-clamp: 3;
		min-width: 0;
		overflow: hidden;
		overflow-wrap: anywhere;
	}
</style>
