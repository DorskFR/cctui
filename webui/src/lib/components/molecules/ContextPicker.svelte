<script lang="ts">
	// Fuzzy multi-select over context items. The kit's Combobox is single-pick,
	// so a pick toggles the item and reopens the listbox.
	import { Badge, Combobox, Input } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import {
		chipsOf,
		dropLast,
		fuzzyRank,
		keyOf,
		togglePick,
		type PickBy,
		type Pickable
	} from './context.logic';

	let {
		items,
		value = $bindable(),
		by = 'name',
		matched = [],
		locked = [],
		label,
		placeholder = m.context_picker_placeholder(),
		disabled = false,
		id,
		onchange
	}: {
		/** Pickable items: memories and prompt templates, never rules. */
		items: Pickable[];
		value: string[];
		by?: PickBy;
		/** Keys that match the target's scope, marked in the list. */
		matched?: string[];
		/** Keys shown as chips the user cannot remove (a profile's pins). */
		locked?: string[];
		label: string;
		placeholder?: string;
		disabled?: boolean;
		id?: string;
		onchange?: (value: string[]) => void;
	} = $props();

	let query = $state('');
	let open = $state(false);
	let index = $state(0);

	const options = $derived(fuzzyRank(query, items));
	const shown = $derived([...locked.filter((k) => !value.includes(k)), ...value]);
	const chips = $derived(chipsOf(shown, items, by));

	function set(next: string[]) {
		value = next;
		onchange?.(next);
	}
	function pick(item: Pickable) {
		const key = keyOf(item, by);
		if (locked.includes(key)) return;
		set(togglePick(value, item, items, by));
	}
	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Backspace' && !query && value.length) {
			e.preventDefault();
			set(dropLast(value, locked));
		} else if (!open && (e.key === 'ArrowDown' || e.key === 'ArrowUp')) {
			e.preventDefault();
			open = true;
		}
	}
	const kindLabel = (kind: string) =>
		kind === 'prompt' ? m.context_kind_prompt() : kind === 'rule' ? m.context_kind_rule() : m.context_kind_memory();
</script>

<div class="picker" class:disabled>
	{#if chips.length}
		<div class="chips">
			{#each chips as c (c.id)}
				{@const key = keyOf(c, by)}
				<Badge
					size="sm"
					tone={c.kind === 'prompt' ? 'accent' : 'neutral'}
					removable={!locked.includes(key) && !disabled}
					onremove={() => set(value.filter((k) => k !== key))}
					title={c.name}
					data-chip={key}
				>
					{c.title}
				</Badge>
			{/each}
		</div>
	{/if}
	<Combobox
		{options}
		bind:open
		bind:index
		{label}
		selectOn={['Enter']}
		getKey={(o) => o.id}
		onselect={pick}
		onclose={(reason) => {
			if (reason === 'select') open = true;
		}}
		empty={m.context_picker_empty()}
	>
		<Input
			{id}
			style="width:100%"
			autocomplete="off"
			spellcheck="false"
			aria-label={label}
			{placeholder}
			{disabled}
			bind:value={query}
			onfocus={() => (open = true)}
			oninput={() => {
				open = true;
				index = 0;
			}}
			{onkeydown}
		/>
		{#snippet option(o)}
			{@const key = keyOf(o, by)}
			<span class="row" data-option={key}>
				<span class="check" aria-hidden="true">{value.includes(key) || locked.includes(key) ? '✓' : ''}</span>
				<Badge size="xs" tone={o.kind === 'prompt' ? 'accent' : 'info'}>{kindLabel(o.kind)}</Badge>
				<span class="title">{o.title}</span>
				<span class="slug">{o.name}</span>
				{#if matched.includes(key)}
					<span class="match">{m.context_picker_matches_cwd()}</span>
				{/if}
			</span>
		{/snippet}
	</Combobox>
</div>

<style>
	.picker {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		min-width: 0;
	}
	.disabled {
		opacity: 0.6;
	}
	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-1);
	}
	.row {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	.check {
		flex: none;
		width: 1em;
		color: var(--ok);
	}
	.title {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.slug {
		flex: none;
		font-family: var(--font-mono);
		font-size: var(--fs-xs);
		color: var(--text-faint);
	}
	.match {
		flex: none;
		margin-left: auto;
		font-size: var(--fs-xs);
		color: var(--ok);
	}
</style>
