<script lang="ts">
	import { Button, Icon } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { clickOutside } from '$lib/clickOutside';
	import { promptHistory } from '$lib/drafts';

	// The discoverable half of spawn prompt recall: ArrowUp in the textarea is
	// invisible, so both spawn branches also get this trigger + list of recent
	// prompts. Picking one goes through the caller's HistoryNav so keyboard and
	// mouse recall share one cursor.
	let { onpick, disabled = false }: { onpick: (value: string) => void; disabled?: boolean } = $props();

	let open = $state(false);
	let entries = $state<string[]>([]);

	function toggle() {
		if (!open) entries = promptHistory.get().slice().reverse();
		open = !open;
	}

	function preview(value: string) {
		const line = value.trim().split('\n')[0];
		return line.length > 120 ? `${line.slice(0, 120)}…` : line;
	}
</script>

<div class="prompt-history" use:clickOutside={() => (open = false)}>
	<Button
		square
		{disabled}
		aria-label={m.spawn_prompt_history()}
		title={m.spawn_prompt_history_hint()}
		aria-haspopup="true"
		aria-expanded={open}
		onclick={toggle}
	>
		<Icon name="clock" size={16} />
	</Button>
	{#if open}
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div
			class="menu"
			role="menu"
			aria-label={m.spawn_prompt_history()}
			tabindex="-1"
			onkeydown={(e) => {
				if (e.key === 'Escape') open = false;
			}}
		>
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
							open = false;
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
		</div>
	{/if}
</div>

<style>
	.prompt-history {
		position: relative;
		display: inline-flex;
		flex: none;
	}
	.menu {
		position: absolute;
		top: calc(100% + var(--sp-1));
		right: 0;
		z-index: 40;
		display: flex;
		flex-direction: column;
		width: min(32rem, 70vw);
		max-height: 18rem;
		overflow-y: auto;
		padding: var(--sp-1);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		background: var(--bg-elevated);
		box-shadow: var(--shadow-lg, 0 8px 24px rgba(0, 0, 0, 0.4));
	}
	.empty {
		margin: 0;
		padding: var(--sp-2);
		color: var(--text-faint);
		font-size: var(--fs-xs);
	}
	.entry {
		display: block;
		width: 100%;
		padding: var(--sp-1) var(--sp-2);
		border: none;
		border-radius: var(--r-sm);
		background: none;
		color: var(--text);
		font-family: var(--font-mono);
		font-size: var(--fs-xs);
		text-align: left;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		cursor: pointer;
	}
	.entry:hover {
		background: var(--bg-elevated-3, var(--bg-elevated-2));
	}
	.clear {
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
