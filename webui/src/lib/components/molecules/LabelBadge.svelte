<script lang="ts">
	import type { Label } from '@bindings/Label';
	import { Badge, Icon, Popover } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import LabelMenu from './LabelMenu.svelte';
	import { labelTint, hueToColor } from '$lib/labels';

	// Session-label strip + picker (CCT-360). Attached labels render as hue-tinted,
	// removable Badges. When `editable`, a shared LabelMenu (the same molecule the
	// list-wide LabelFilter uses) provides the popover body: typing filters the
	// existing labels and, when nothing matches, offers a "Create" affordance; each
	// row toggles attach/detach and the pencil opens LabelMenu's rename/recolor/
	// delete modal. This wrapper owns only the badge strip + the `tag` Popover
	// trigger.
	//
	// Recolor/rename go through PATCH /labels/{id} (`onUpdate`), keyed on id so a
	// rename never orphans the old name. Create still uses POST /labels.
	let {
		labels,
		editable = false,
		allLabels = [],
		onCreate,
		onAttach,
		onDetach,
		onUpdate,
		onDelete
	}: {
		labels: Label[];
		editable?: boolean;
		allLabels?: Label[];
		onCreate?: (name: string, color: string) => Promise<Label>;
		onAttach?: (labelId: string) => void | Promise<void>;
		onDetach?: (labelId: string) => void | Promise<void>;
		onUpdate?: (labelId: string, patch: { name?: string; color?: string }) => Promise<Label>;
		onDelete?: (labelId: string) => void | Promise<void>;
	} = $props();

	let busy = $state(false);

	const attachedIds = $derived(new Set(labels.map((l) => l.id)));

	async function toggleExisting(l: Label) {
		if (busy) return;
		busy = true;
		try {
			if (attachedIds.has(l.id)) await onDetach?.(l.id);
			else await onAttach?.(l.id);
		} finally {
			busy = false;
		}
	}

	async function createAndAttach(name: string) {
		if (!name || busy || !onCreate) return;
		busy = true;
		try {
			const label = await onCreate(name, hueToColor(null));
			if (!attachedIds.has(label.id)) await onAttach?.(label.id);
		} finally {
			busy = false;
		}
	}
</script>

{#if labels.length > 0 || editable}
	<!-- Swallow clicks/keys so interacting with labels never taps the parent
	     session card (the row is a clickable Card). svelte-ignore: the span is a
	     pure event boundary, not itself an interactive control. -->
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<span
		class="labels"
		onpointerdown={(e) => e.stopPropagation()}
		onclick={(e) => e.stopPropagation()}
		onkeydown={(e) => e.stopPropagation()}
	>
		{#each labels as l (l.id)}
			<Badge
				class="label"
				style="{labelTint(l)};border-radius:var(--r-sm)"
				removable={editable}
				onremove={() => {
					if (!busy) onDetach?.(l.id);
				}}
			>
				<span class="name">{l.name}</span>
			</Badge>
		{/each}

		{#if editable}
			<span class="add">
				<Popover label={m.sessions_edit_labels()} placement="bottom-start" triggerClass="tag-trigger">
					{#snippet trigger()}<Icon name="tag" />{/snippet}
					<LabelMenu
						labels={allLabels}
						selectedIds={attachedIds}
						cap={5}
						{busy}
						onToggle={toggleExisting}
						onCreate={onCreate ? createAndAttach : undefined}
						{onUpdate}
						{onDelete}
					/>
				</Popover>
			</span>
		{/if}
	</span>
{/if}

<style>
	.labels {
		display: inline-flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-1);
	}
	.name {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	/* `tag` trigger: a small ghost icon-button reading clearly as "labels". */
	.add :global(.tag-trigger) {
		min-width: 1.5rem;
		min-height: 1.5rem;
		padding: var(--sp-1);
		color: var(--text-muted);
	}
	.add :global(.tag-trigger:hover) {
		color: var(--text);
		background: var(--bg-elevated-2);
	}
</style>
