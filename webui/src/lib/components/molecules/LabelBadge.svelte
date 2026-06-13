<script lang="ts">
	import type { Label } from '@bindings/Label';
	import { Badge, Icon, IconButton, Input, Popover } from '@dorsk/tsumikit';
	import ColorPicker from '$lib/components/molecules/ColorPicker.svelte';
	import Swatch from '$lib/components/atoms/Swatch.svelte';
	import { LABEL_HUES, labelHue, labelTint, storedHue, hueToColor } from '$lib/labels';

	// Session-label strip + picker (CCT-360, reworked onto tsumikit). Renders the
	// attached labels as hue-tinted Badges, each with a colored dot that opens the
	// shared ColorPicker to recolor the tag. When `editable`, a dashed-circle `+`
	// opens a Popover to toggle existing tags or create a new one (name + color).
	//
	// Recolor reuses POST /labels — it is get-or-create-by-name and refreshes the
	// color, so re-posting the same name with a new hue recolors it everywhere.
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

	let name = $state('');
	let newHue = $state<number | null>(null);
	let busy = $state(false);

	const attachedIds = $derived(new Set(labels.map((l) => l.id)));

	async function recolor(label: Label, hue: number | null) {
		if (busy || !onCreate) return;
		busy = true;
		try {
			await onCreate(label.name, hueToColor(hue));
		} finally {
			busy = false;
		}
	}

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

	async function createAndAttach(e: SubmitEvent) {
		e.preventDefault();
		const trimmed = name.trim();
		if (!trimmed || busy || !onCreate) return;
		busy = true;
		try {
			const label = await onCreate(trimmed, hueToColor(newHue));
			if (!attachedIds.has(label.id)) await onAttach?.(label.id);
			name = '';
			newHue = null;
		} finally {
			busy = false;
		}
	}
</script>

{#snippet dot(l: Label)}
	<span class="dot" style="background:hsl({labelHue(l)} 60% 50%)"></span>
{/snippet}

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
			<!-- The chip is just the tinted, square, removable label — recolor lives in
			     the add-popover (per-label ColorPicker), NOT inside the Badge. -->
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
				<Popover label="Add label" placement="bottom-start">
					{#snippet trigger()}<Icon name="plus" label="Add label" />{/snippet}
					{#if allLabels.length > 0}
						<div class="opts">
							{#each allLabels as l (l.id)}
								<div class="opt-row">
									<ColorPicker
										value={storedHue(l.color)}
										hues={LABEL_HUES}
										label={`Recolor ${l.name}`}
										onchange={(h) => recolor(l, h)}
									>
										{#snippet trigger()}{@render dot(l)}{/snippet}
									</ColorPicker>
									<button
										type="button"
										class="opt"
										aria-pressed={attachedIds.has(l.id)}
										disabled={busy}
										onclick={() => toggleExisting(l)}
									>
										<span class="check">{attachedIds.has(l.id) ? '✓' : ''}</span>
										<span class="opt-name">{l.name}</span>
									</button>
								</div>
							{/each}
						</div>
						<div class="sep"></div>
					{/if}

					<form class="create" onsubmit={createAndAttach}>
						<div class="create-row">
							<ColorPicker
								value={newHue}
								hues={LABEL_HUES}
								label="New label color"
								onchange={(h) => (newHue = h)}
							>
								{#snippet trigger()}<Swatch hue={newHue} />{/snippet}
							</ColorPicker>
							<Input placeholder="New label…" bind:value={name} maxlength={40} />
						</div>
						<IconButton
							icon="plus"
							variant="primary"
							label="Create and attach label"
							disabled={busy || !name.trim()}
							onclick={() => {}}
							type="submit"
						/>
					</form>
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
	/* The colored dot is the recolor handle inside each tinted badge. */
	.dot {
		display: inline-block;
		width: 0.6rem;
		height: 0.6rem;
		border-radius: 50%;
		flex: none;
		box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.25);
	}
	.name {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	/* `+` trigger: reuse the Popover's ghost-icon button, restyled as the dotted
	   circle (Swatch's "Auto" affordance) the design calls for. */
	.add :global(.pop-trigger) {
		width: 1.25rem;
		height: 1.25rem;
		min-width: 0;
		min-height: 0;
		padding: 0;
		background: transparent;
		border: 1px dashed var(--border-strong);
		border-radius: 50%;
		color: var(--text-muted);
	}
	.add :global(.pop-trigger:hover:not(:disabled)) {
		background: transparent;
		color: var(--text);
		border-color: var(--accent);
	}

	.opts {
		display: flex;
		flex-direction: column;
		gap: 0.1rem;
		max-height: 11rem;
		overflow-y: auto;
	}
	.opt-row {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		padding: 0 var(--sp-1);
	}
	.opt {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		flex: 1;
		min-width: 0;
		padding: var(--sp-1);
		border: none;
		background: none;
		color: var(--text);
		cursor: pointer;
		border-radius: var(--r-sm);
		font-size: var(--fs-sm);
		text-align: left;
	}
	.opt:hover {
		background: var(--bg);
	}
	.check {
		width: 1rem;
		color: var(--accent);
	}
	.opt-name {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.sep {
		height: 1px;
		background: var(--border);
		margin: var(--sp-1) 0;
	}
	.create {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-1);
	}
	.create-row {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
</style>
