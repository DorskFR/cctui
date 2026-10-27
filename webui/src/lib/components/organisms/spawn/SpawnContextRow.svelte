<script lang="ts">
	import { untrack } from 'svelte';
	import { Badge, Field, Text, Toggle } from '@dorsk/tsumikit';
	import ContextPicker from '$lib/components/molecules/ContextPicker.svelte';
	import { sameList } from '$lib/components/molecules/context.logic';
	import { useContextItems, useResolvedContext } from '$lib/queries';
	import { m } from '$lib/paraglide/messages';

	let {
		items = $bindable(),
		pins = $bindable([]),
		machineId,
		workingDir,
		labels,
		adapter,
		pinnedIds = []
	}: {
		/** Picked item names. */
		items: string[];
		/** Names of the profile's pinned items, sent alongside the picks. */
		pins?: string[];
		machineId: string;
		workingDir: string;
		labels: string[];
		adapter: string;
		pinnedIds?: string[];
	} = $props();

	const all = useContextItems();
	const resolved = useResolvedContext(() => ({
		machine_id: machineId,
		working_dir: workingDir,
		labels,
		adapter
	}));

	const pickable = $derived(
		(all.data ?? []).filter((i) => i.enabled && (i.kind === 'memory' || i.kind === 'prompt'))
	);
	const rules = $derived(resolved.data?.rules ?? []);
	const matched = $derived(resolved.data?.memories.map((i) => i.name) ?? []);
	const locked = $derived(pickable.filter((i) => pinnedIds.includes(i.id)).map((i) => i.name));
	let showRules = $state(false);

	// Writing an unchanged list still dirties the parent form, which re-runs the
	// resolve query and this effect: only write real changes.
	$effect(() => {
		const next = locked;
		untrack(() => {
			if (!sameList(pins, next)) pins = next;
		});
	});
</script>

<Field label={m.spawn_context_label()} for="sp-context">
	<div class="ctx">
		{#if rules.length}
			<div class="rules">
				<Toggle
					pill
					pressed={showRules}
					aria-expanded={showRules}
					onclick={() => (showRules = !showRules)}
					data-rules-count={rules.length}
				>
					{m.context_rules_apply({ count: rules.length })}
				</Toggle>
				{#if showRules}
					{#each rules as r (r.id)}
						<Badge size="sm" title={r.name}>{r.title}</Badge>
					{/each}
				{/if}
			</div>
		{/if}
		<ContextPicker
			id="sp-context"
			items={pickable}
			bind:value={items}
			{matched}
			{locked}
			label={m.spawn_context_label()}
		/>
		{#if !pickable.length}
			<Text size="xs" tone="faint">{m.spawn_context_none()}</Text>
		{/if}
	</div>
</Field>

<style>
	.ctx {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.rules {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-1);
	}
</style>
