<script lang="ts">
	import type { Label } from '@bindings/Label';
	import { Input } from '@dorsk/tsumikit';
	import { LABEL_COLORS, labelTextColor } from '$lib/labels';

	// Label strip + picker (CCT-360). Renders the attached labels as colored
	// chips and, when `editable`, a `+` trigger that opens a dropdown to toggle
	// existing labels or create a new one (name + color picker).
	//
	// All inline affordances are <span role="button">, never <button>, so the
	// whole component is valid INSIDE a parent <button> (the session card). The
	// popover itself holds form controls, so when `portalMenu` is set it is
	// teleported to <body> (escaping the card button) and positioned under the
	// trigger.
	let {
		labels,
		editable = false,
		allLabels = [],
		portalMenu = false,
		onCreate,
		onAttach,
		onDetach
	}: {
		labels: Label[];
		editable?: boolean;
		allLabels?: Label[];
		/** Teleport the popover to <body> (needed when nested in a <button>). */
		portalMenu?: boolean;
		onCreate?: (name: string, color: string) => Promise<Label>;
		onAttach?: (labelId: string) => void | Promise<void>;
		onDetach?: (labelId: string) => void | Promise<void>;
	} = $props();

	let open = $state(false);
	let name = $state('');
	let color = $state(LABEL_COLORS[0]);
	let busy = $state(false);
	let rootEl = $state<HTMLElement>();
	let addEl = $state<HTMLElement>();
	let menuEl = $state<HTMLElement>();
	// Non-portaled menus are positioned with CSS (top:100%); flip to bottom:100%
	// when opening below would overflow the viewport. Portaled menus handle this
	// inline in the `portal` action instead.
	let flipUp = $state(false);

	const attachedIds = $derived(new Set(labels.map((l) => l.id)));

	// Close on any pointer outside BOTH the inline strip and the (possibly
	// portaled) menu. Installed only while open.
	$effect(() => {
		if (!open) return;
		const handler = (e: Event) => {
			const t = e.target as Node;
			if (rootEl?.contains(t) || menuEl?.contains(t)) return;
			open = false;
		};
		document.addEventListener('pointerdown', handler, true);
		return () => document.removeEventListener('pointerdown', handler, true);
	});

	// Decide flip direction for the CSS-positioned (non-portaled) menu once it is
	// rendered. Runs whenever the menu opens.
	$effect(() => {
		if (!open || portalMenu || !menuEl || !addEl) {
			flipUp = false;
			return;
		}
		const r = addEl.getBoundingClientRect();
		const mh = menuEl.offsetHeight || 0;
		flipUp = r.bottom + 4 + mh > window.innerHeight && r.top - 4 - mh >= 0;
	});

	// Portal + position the popover under the trigger; reposition on scroll/resize.
	function portal(node: HTMLElement) {
		if (!portalMenu) return;
		document.body.appendChild(node);
		const place = () => {
			if (!addEl) return;
			const r = addEl.getBoundingClientRect();
			node.style.position = 'fixed';
			const mw = node.offsetWidth || 220;
			const mh = node.offsetHeight || 0;
			// Flip up when opening below would overflow the viewport bottom and
			// there is room above (e.g. trigger already near the bottom edge).
			const below = r.bottom + 4;
			const flipUp = below + mh > window.innerHeight && r.top - 4 - mh >= 0;
			node.style.top = flipUp ? `${r.top - 4 - mh}px` : `${below}px`;
			node.style.left = `${Math.max(8, Math.min(r.left, window.innerWidth - mw - 8))}px`;
			node.style.zIndex = '1000';
		};
		place();
		window.addEventListener('scroll', place, true);
		window.addEventListener('resize', place);
		return {
			destroy() {
				window.removeEventListener('scroll', place, true);
				window.removeEventListener('resize', place);
				node.remove();
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
	<span class="labels" bind:this={rootEl}>
		{#each labels as l (l.id)}
			<span class="chip" style="background:{l.color};color:{labelTextColor(l.color)}" title={l.name}>
				{l.name}
				{#if editable}
					<span
						class="chip-x"
						role="button"
						tabindex="0"
						aria-label={`Remove ${l.name}`}
						onpointerdown={(e) => e.stopPropagation()}
						onclick={(e) => removeChip(e, l)}
						onkeydown={(e) => {
							if (e.key === 'Enter' || e.key === ' ') removeChip(e, l);
						}}>×</span
					>
				{/if}
			</span>
		{/each}

		{#if editable}
			<span
				class="add"
				bind:this={addEl}
				role="button"
				tabindex="0"
				title="Add label"
				aria-label="Add label"
				aria-haspopup="true"
				aria-expanded={open}
				onpointerdown={(e) => e.stopPropagation()}
				onclick={(e) => {
					e.stopPropagation();
					open = !open;
				}}
				onkeydown={(e) => {
					if (e.key === 'Enter' || e.key === ' ') {
						e.preventDefault();
						open = !open;
					}
				}}>+</span
			>

			{#if open}
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<div
					class="menu"
					class:up={flipUp}
					bind:this={menuEl}
					use:portal
					onpointerdown={(e) => e.stopPropagation()}
				>
					{#if allLabels.length > 0}
						<div class="menu-list">
							{#each allLabels as l (l.id)}
								<button
									type="button"
									class="opt"
									aria-pressed={attachedIds.has(l.id)}
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
						<Input class="name-in" placeholder="New label…" bind:value={name} maxlength={40} />
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
						<button type="submit" class="create-btn" disabled={busy || !name.trim()}>Add</button>
					</form>
				</div>
			{/if}
		{/if}
	</span>
{/if}

<style>
	.labels {
		position: relative;
		display: inline-flex;
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
		cursor: pointer;
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
		color: var(--text-muted);
		cursor: pointer;
		font-size: var(--fs-sm);
		line-height: 1;
	}
	.add:hover {
		color: var(--text);
		border-color: var(--accent);
	}
	/* The popover. When portaled to <body> it keeps this scoped class (Svelte
	   scoping is class-based and survives the DOM move), so the styling holds. */
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
		text-align: left;
		white-space: normal;
	}
	/* Flip the CSS-positioned menu above the trigger when it would otherwise
	   overflow the viewport bottom. Inline-styled portaled menus ignore this. */
	.menu.up {
		top: auto;
		bottom: 100%;
		margin-top: 0;
		margin-bottom: var(--sp-1);
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
