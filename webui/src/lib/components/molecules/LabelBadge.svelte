<script lang="ts">
	import type { Label } from '@bindings/Label';
	import { Badge, Dot, Icon, Popover } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import LabelMenu from './LabelMenu.svelte';
	import { labelHue, labelTint, hueToColor } from '$lib/labels';

	// Session-label strip + picker. Attached labels render as hue-tinted,
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
	<!-- Swallow clicks so tapping a label chip never opens the parent session
	     Card (keys are already ignored there when they come from a nested
	     control). svelte-ignore: the span is a pure event boundary. -->
	<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
	<span class="labels" onpointerdown={(e) => e.stopPropagation()} onclick={(e) => e.stopPropagation()}>
		{#each labels as l (l.id)}
			<span class="full">
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
			</span>
		{/each}
		{#if labels.length > 0}
			<span class="dots" title={m.sessions_labels_collapsed_title({ names: labels.map((l) => l.name).join(', ') })}>
				{#each labels as l (l.id)}<Dot color="hsl({labelHue(l)} var(--mach-border-sl))" />{/each}
			</span>
		{/if}

		{#if editable}
			<span class="add">
				<Popover label={m.sessions_edit_labels()} placement="bottom-start" bare triggerClass="tag-trigger">
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
	.full {
		display: contents;
	}
	.name {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	/* Cramped `sess-card` host: the chips collapse to coloured dots. */
	.dots {
		display: none;
		align-items: center;
		gap: var(--sp-1);
	}
	@container sess-card (max-width: 20rem) {
		.full {
			display: none;
		}
		.dots {
			display: inline-flex;
		}
	}
	@container sess-row (max-width: 40rem) {
		.full {
			display: none;
		}
		.dots {
			display: inline-flex;
		}
	}
	/* `tag` trigger: a small ghost icon-button reading clearly as "labels". */
	/* .bare in the selector: the kit's bare reset ties on specificity and loads later. */
	.add :global(.tag-trigger.bare) {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		min-width: 1.5rem;
		min-height: 1.5rem;
		padding: var(--sp-1);
		border-radius: var(--r-sm);
		color: var(--text-muted);
	}
	.add :global(.tag-trigger.bare:hover) {
		color: var(--text);
		background: var(--bg-elevated-2);
	}
</style>
