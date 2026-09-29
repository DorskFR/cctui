<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { Button, Field, FilterSearchBar, Icon, Popover, Toggle, type Schema } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import type { SplitDirection } from '$lib/tiles';

	let {
		schema,
		rawQuery = $bindable(''),
		candidates,
		count,
		max,
		splitDirection,
		onadd,
		onnew,
		onsplit
	}: {
		schema: Schema;
		/** The picker's raw query; the page owns it so it can narrow `candidates`. */
		rawQuery?: string;
		/** Non-archived sessions not already tiled, already narrowed by the query. */
		candidates: SessionListItem[];
		count: number;
		max: number;
		splitDirection: SplitDirection;
		onadd: (id: string) => void;
		onnew: () => void;
		onsplit: (d: SplitDirection) => void;
	} = $props();

	const pickerId = $props.id();
</script>

<div class="tbar" data-journey="tiles-toolbar">
	<Popover label={m.tiles_add()} placement="bottom-start" variant="default" panelStyle="width:24rem">
		{#snippet trigger()}
			<Icon name="plus" size={16} />{m.tiles_add()}
		{/snippet}
		<div class="picker">
			<label for={pickerId} class="sr-only">{m.tiles_add_search()}</label>
			<Field for={pickerId}>
				<FilterSearchBar {schema} size="sm" bind:value={rawQuery} placeholder={m.tiles_add_search()} />
			</Field>
			<ul class="cands">
				{#each candidates.slice(0, 30) as s (s.id)}
					<li>
						<Button variant="ghost" onclick={() => onadd(s.id)} style="justify-content:flex-start;width:100%">
							{s.name || s.working_dir}
						</Button>
					</li>
				{:else}
					<li class="none">{m.tiles_add_none()}</li>
				{/each}
			</ul>
		</div>
	</Popover>
	<Button onclick={onnew}>{m.tiles_new()}</Button>
	<span class="spacer"></span>
	<span class="count">{m.tiles_count({ n: count, max })}</span>
	<Toggle
		pressed={splitDirection === 'horizontal'}
		title={m.tiles_split_title()}
		onclick={() => onsplit(splitDirection === 'horizontal' ? 'vertical' : 'horizontal')}
		>{splitDirection === 'horizontal' ? '⬓' : '◨'}</Toggle
	>
</div>

<style>
	.tbar {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
		border-bottom: 1px solid var(--border-strong);
		background: var(--bg-elevated);
		flex: none;
	}
	.spacer {
		flex: 1;
	}
	.count {
		color: var(--text-muted);
		font-variant-numeric: tabular-nums;
		font-size: var(--fs-sm);
	}
	.picker {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.cands {
		list-style: none;
		margin: 0;
		padding: 0;
		max-height: 18rem;
		overflow-y: auto;
	}
	.none {
		color: var(--text-muted);
		padding: var(--sp-2);
	}
</style>
