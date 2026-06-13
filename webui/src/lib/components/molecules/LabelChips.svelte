<script lang="ts">
	import type { Label } from '@bindings/Label';
	import { Input } from '@dorsk/tsumikit';
	import { LABEL_COLORS, labelTextColor } from '$lib/labels';

	// Per-session label strip (CCT-360): renders the session's colored labels as
	// chips and, when `editable`, an inline "+" that opens a small dropdown to
	// pick existing labels or create a new one (name + color picker). Read-only
	// callers (e.g. subagent rows) just get the chips.
	let {
		labels,
		editable = false,
		allLabels = [],
		onCreate,
		onAttach,
		onDetach
	}: {
		/** Labels currently attached to this session. */
		labels: Label[];
		editable?: boolean;
		/** Every label known to the server — the picker's existing-label list. */
		allLabels?: Label[];
		/** get-or-create a label by name; resolves to the label so we can attach it. */
		onCreate?: (name: string, color: string) => Promise<Label>;
		onAttach?: (labelId: string) => void | Promise<void>;
		onDetach?: (labelId: string) => void | Promise<void>;
	} = $props();

	let open = $state(false);
	let name = $state('');
	let color = $state(LABEL_COLORS[0]);
	let busy = $state(false);

	const attachedIds = $derived(new Set(labels.map((l) => l.id)));

	// Close the popover on any outside pointer (mirrors the sessions page).
	function clickOutside(node: HTMLElement, onOutside: () => void) {
		const handler = (e: Event) => {
			if (!node.contains(e.target as Node)) onOutside();
		};
		document.addEventListener('pointerdown', handler, true);
		return {
			destroy() {
				document.removeEventListener('pointerdown', handler, true);
			}
		};
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
			const label = await onCreate(trimmed, color);
			if (!attachedIds.has(label.id)) await onAttach?.(label.id);
			name = '';
		} finally {
			busy = false;
		}
	}

	async function removeChip(e: Event, l: Label) {
		e.stopPropagation();
		if (busy) return;
		busy = true;
		try {
			await onDetach?.(l.id);
		} finally {
			busy = false;
		}
	}
</script>

{#if labels.length > 0 || editable}
	<div class="labels" use:clickOutside={() => (open = false)}>
		{#each labels as l (l.id)}
			<span
				class="chip"
				style="background:{l.color};color:{labelTextColor(l.color)}"
				title={l.name}
			>
				{l.name}
				{#if editable}
					<button
						type="button"
						class="chip-x"
						aria-label={`Remove ${l.name}`}
						onpointerdown={(e) => e.stopPropagation()}
						onclick={(e) => removeChip(e, l)}>×</button
					>
				{/if}
			</span>
		{/each}

		{#if editable}
			<button
				type="button"
				class="add"
				title="Add label"
				aria-label="Add label"
				aria-haspopup="true"
				aria-expanded={open}
				onpointerdown={(e) => e.stopPropagation()}
				onclick={(e) => {
					e.stopPropagation();
					open = !open;
				}}>+</button
			>

			{#if open}
				<div class="menu" aria-label="Labels">
					{#if allLabels.length > 0}
						<div class="menu-list">
							{#each allLabels as l (l.id)}
								<button
									type="button"
									class="opt"
									role="menuitemcheckbox"
									aria-checked={attachedIds.has(l.id)}
									disabled={busy}
									onclick={() => toggleExisting(l)}
								>
									<span class="opt-check">{attachedIds.has(l.id) ? '✓' : ''}</span>
									<span class="swatch" style="background:{l.color}"></span>
									<span class="opt-name">{l.name}</span>
								</button>
							{/each}
						</div>
						<div class="sep"></div>
					{/if}

					<form class="create" onsubmit={createAndAttach}>
						<Input
							class="name-in"
							placeholder="New label…"
							bind:value={name}
							maxlength={40}
						/>
						<div class="swatches">
							{#each LABEL_COLORS as c (c)}
								<button
									type="button"
									class="sw"
									class:on={color === c}
									style="background:{c}"
									aria-label={`Color ${c}`}
									onclick={() => (color = c)}
								></button>
							{/each}
							<label class="sw custom" style="background:{color}" title="Custom color">
								<input type="color" bind:value={color} aria-label="Custom color" />
							</label>
						</div>
						<button type="submit" class="create-btn" disabled={busy || !name.trim()}
							>Add</button
						>
					</form>
				</div>
			{/if}
		{/if}
	</div>
{/if}

<style>
	.labels {
		position: relative;
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-1);
	}
	.chip {
		display: inline-flex;
		align-items: center;
		gap: 0.15rem;
		padding: 0.05rem var(--sp-2);
		border-radius: var(--r-pill, 999px);
		font-size: var(--fs-xs);
		font-weight: var(--fw-semibold);
		line-height: 1.4;
		white-space: nowrap;
	}
	.chip-x {
		border: none;
		background: none;
		color: inherit;
		cursor: pointer;
		padding: 0;
		font-size: 0.9rem;
		line-height: 1;
		opacity: 0.75;
	}
	.chip-x:hover {
		opacity: 1;
	}
	.add {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 1.1rem;
		height: 1.1rem;
		border-radius: var(--r-pill, 999px);
		border: 1px dashed var(--border-strong);
		background: none;
		color: var(--text-muted);
		cursor: pointer;
		font-size: var(--fs-sm);
		line-height: 1;
		padding: 0;
	}
	.add:hover {
		color: var(--text);
		border-color: var(--accent);
	}
	.menu {
		position: absolute;
		top: 100%;
		left: 0;
		margin-top: var(--sp-1);
		z-index: 20;
		min-width: 13rem;
		background: var(--bg-elevated);
		border: 1px solid var(--border);
		border-radius: var(--r-md);
		box-shadow: var(--shadow-md, 0 4px 16px rgba(0, 0, 0, 0.25));
		padding: var(--sp-2);
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.menu-list {
		display: flex;
		flex-direction: column;
		max-height: 11rem;
		overflow-y: auto;
	}
	.opt {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		width: 100%;
		padding: var(--sp-1) var(--sp-2);
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
	.opt-check {
		width: 1rem;
		color: var(--accent);
	}
	.opt-name {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.swatch {
		width: 0.8rem;
		height: 0.8rem;
		border-radius: var(--r-sm);
		flex: none;
	}
	.sep {
		height: 1px;
		background: var(--border);
	}
	.create {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.swatches {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-1);
		align-items: center;
	}
	.sw {
		width: 1.1rem;
		height: 1.1rem;
		border-radius: var(--r-sm);
		border: 2px solid transparent;
		cursor: pointer;
		padding: 0;
	}
	.sw.on {
		border-color: var(--text);
	}
	.sw.custom {
		position: relative;
		display: inline-flex;
		border: 1px solid var(--border-strong);
		overflow: hidden;
	}
	.sw.custom input {
		position: absolute;
		inset: 0;
		opacity: 0;
		cursor: pointer;
		border: none;
		padding: 0;
	}
	.create-btn {
		align-self: flex-start;
		padding: 0.15rem var(--sp-3);
		border-radius: var(--r-sm);
		border: 1px solid var(--accent);
		background: var(--accent);
		color: var(--bg);
		cursor: pointer;
		font-size: var(--fs-sm);
		font-weight: var(--fw-semibold);
	}
	.create-btn:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}
</style>
