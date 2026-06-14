<script lang="ts">
	import type { Label } from '@bindings/Label';
	import { Badge, Icon, Input, Popover } from '@dorsk/tsumikit';
	import Swatch from '$lib/components/atoms/Swatch.svelte';
	import { LABEL_HUES, labelHue, labelTint, storedHue, hueToColor } from '$lib/labels';

	// Session-label strip + picker (CCT-360, reworked CCT-? onto a single
	// filter-and-pick popover). Attached labels render as hue-tinted, removable
	// Badges. When `editable`, a `tag` trigger opens ONE popover that is a combined
	// filter/create box: a text input at the top filters the existing labels and,
	// when nothing matches, becomes a "Create" affordance. Each row toggles
	// attach/detach; the pencil expands an INLINE hue strip (in flow — no nested
	// popover, so nothing is clipped behind the input the way the old ColorPicker
	// pop-in-pop was) to recolor that label.
	//
	// Recolor reuses POST /labels — get-or-create-by-name that refreshes the color,
	// so re-posting the same name with a new hue recolors it everywhere.
	let {
		labels,
		editable = false,
		allLabels = [],
		onCreate,
		onAttach,
		onDetach
	}: {
		labels: Label[];
		editable?: boolean;
		allLabels?: Label[];
		onCreate?: (name: string, color: string) => Promise<Label>;
		onAttach?: (labelId: string) => void | Promise<void>;
		onDetach?: (labelId: string) => void | Promise<void>;
	} = $props();

	let q = $state('');
	let busy = $state(false);
	// Which label's inline hue strip is expanded (null = none).
	let editingId = $state<string | null>(null);

	const attachedIds = $derived(new Set(labels.map((l) => l.id)));
	const query = $derived(q.trim());
	const filtered = $derived(
		query ? allLabels.filter((l) => l.name.toLowerCase().includes(query.toLowerCase())) : allLabels
	);
	const exactMatch = $derived(
		allLabels.find((l) => l.name.toLowerCase() === query.toLowerCase()) ?? null
	);
	const showCreate = $derived(!!query && !exactMatch);

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

	async function recolor(l: Label, hue: number | null) {
		if (busy || !onCreate) return;
		busy = true;
		try {
			await onCreate(l.name, hueToColor(hue));
		} finally {
			busy = false;
			editingId = null;
		}
	}

	async function createAndAttach() {
		if (!query || busy || !onCreate) return;
		busy = true;
		try {
			const label = await onCreate(query, hueToColor(null));
			if (!attachedIds.has(label.id)) await onAttach?.(label.id);
			q = '';
		} finally {
			busy = false;
		}
	}

	// Enter in the filter box: toggle the single exact match, else create.
	function onSubmit(e: SubmitEvent) {
		e.preventDefault();
		if (exactMatch) toggleExisting(exactMatch);
		else createAndAttach();
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
				<Popover
					label="Edit labels"
					placement="bottom-start"
					triggerClass="tag-trigger"
					onclose={() => {
						q = '';
						editingId = null;
					}}
				>
					{#snippet trigger()}<Icon name="tag" />{/snippet}

					<form class="filter" onsubmit={onSubmit}>
						<Icon name="search" />
						<Input placeholder="Filter or create…" bind:value={q} maxlength={40} />
					</form>

					<div class="list">
						{#each filtered as l (l.id)}
							<div class="row">
								<button
									type="button"
									class="opt"
									aria-pressed={attachedIds.has(l.id)}
									disabled={busy}
									onclick={() => toggleExisting(l)}
								>
									<span class="check">
										{#if attachedIds.has(l.id)}<Icon name="check" />{/if}
									</span>
									<span class="dot" style="background:hsl({labelHue(l)} 60% 50%)"></span>
									<span class="opt-name">{l.name}</span>
								</button>
								<button
									type="button"
									class="edit"
									class:on={editingId === l.id}
									aria-label={`Recolor ${l.name}`}
									aria-expanded={editingId === l.id}
									disabled={busy}
									onclick={() => (editingId = editingId === l.id ? null : l.id)}
								>
									<Icon name="edit" />
								</button>
							</div>
							{#if editingId === l.id}
								<div class="hues" role="radiogroup" aria-label={`Color for ${l.name}`}>
									<Swatch
										hue={null}
										active={storedHue(l.color) == null}
										title="Auto (name hash)"
										aria-label="Auto color"
										onclick={() => recolor(l, null)}>A</Swatch
									>
									{#each LABEL_HUES as h (h)}
										<Swatch
											hue={h}
											active={storedHue(l.color) === h}
											title={`Hue ${h}`}
											aria-label={`Hue ${h}`}
											onclick={() => recolor(l, h)}
										/>
									{/each}
								</div>
							{/if}
						{/each}

						{#if showCreate}
							<button type="button" class="opt create" disabled={busy} onclick={createAndAttach}>
								<span class="check" aria-hidden="true">+</span>
								<span class="opt-name">Create “{query}”</span>
							</button>
						{/if}

						{#if filtered.length === 0 && !showCreate}
							<p class="empty">No labels yet — type to create one.</p>
						{/if}
					</div>
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

	/* Filter/create box pinned at the top of the panel. */
	.filter {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		padding: var(--sp-1) var(--sp-2);
		color: var(--text-muted);
	}
	.filter :global(input) {
		flex: 1;
		min-width: 0;
	}

	.list {
		display: flex;
		flex-direction: column;
		gap: 0.05rem;
		max-height: 14rem;
		overflow-y: auto;
		margin-top: var(--sp-1);
	}
	.row {
		display: flex;
		align-items: center;
		gap: var(--sp-1);
	}
	.opt {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		flex: 1;
		min-width: 0;
		padding: var(--sp-1) var(--sp-2);
		border: none;
		background: none;
		color: var(--text);
		cursor: pointer;
		border-radius: var(--r-sm);
		font-size: var(--fs-sm);
		text-align: left;
	}
	.opt:hover:not(:disabled),
	.edit:hover:not(:disabled) {
		background: var(--bg-elevated-2);
	}
	.check {
		display: inline-flex;
		width: 0.9rem;
		flex: none;
		color: var(--accent);
	}
	.dot {
		display: inline-block;
		width: 0.6rem;
		height: 0.6rem;
		border-radius: 50%;
		flex: none;
		box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.25);
	}
	.opt-name {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.edit {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		flex: none;
		padding: var(--sp-1);
		border: none;
		background: none;
		color: var(--text-muted);
		cursor: pointer;
		border-radius: var(--r-sm);
	}
	.edit.on {
		color: var(--accent);
	}
	/* Inline hue strip — lives in flow, pushing the list down. Never overlays or
	   gets clipped (the old nested ColorPicker popover did). */
	.hues {
		display: flex;
		flex-wrap: wrap;
		gap: 4px;
		padding: var(--sp-1) var(--sp-2) var(--sp-2) calc(0.9rem + 2 * var(--sp-2));
	}
	.create {
		color: var(--text-muted);
	}
	.empty {
		margin: 0;
		padding: var(--sp-2);
		color: var(--text-muted);
		font-size: var(--fs-sm);
	}
</style>
