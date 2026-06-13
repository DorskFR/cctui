<script lang="ts">
	import type { Snippet } from 'svelte';
	import { browser } from '$app/environment';
	import IconButton from '$lib/components/molecules/IconButton.svelte';

	let {
		title,
		onclose,
		body,
		footer,
		// When set, the sheet is horizontally resizable on desktop (CCT-279 item 3)
		// and the chosen width persists under this localStorage key. Min width is
		// the default 34rem (so it never shrinks below the old fixed size).
		resizeKey
	}: {
		title: string;
		onclose: () => void;
		body: Snippet;
		footer?: Snippet;
		resizeKey?: string;
	} = $props();

	// Default (and minimum) sheet width: 34rem at the 16px base = 544px.
	const MIN_W = 544;
	const MAX_W = 1100;

	function loadWidth(): number | null {
		if (!browser || !resizeKey) return null;
		const n = Number(localStorage.getItem(resizeKey));
		return Number.isFinite(n) && n >= MIN_W ? Math.min(n, MAX_W) : null;
	}
	let width = $state<number | null>(loadWidth());

	let resizing = $state(false);
	let startX = 0;
	let startW = 0;
	function startResize(e: PointerEvent) {
		resizing = true;
		startX = e.clientX;
		// Resize symmetrically from the centered sheet: dragging the right edge by
		// `dx` widens the sheet by `2*dx` (the modal is centered), so the cursor
		// tracks the edge.
		startW = (e.currentTarget as HTMLElement).closest('.sheet')!.getBoundingClientRect().width;
		(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
		e.preventDefault();
	}
	function onResize(e: PointerEvent) {
		if (!resizing) return;
		const next = startW + (e.clientX - startX) * 2;
		width = Math.round(Math.max(MIN_W, Math.min(next, MAX_W, window.innerWidth - 32)));
	}
	function endResize(e: PointerEvent) {
		if (!resizing) return;
		resizing = false;
		try {
			(e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
		} catch {
			/* already released */
		}
		if (browser && resizeKey && width != null) localStorage.setItem(resizeKey, String(width));
	}

	function onkey(e: KeyboardEvent) {
		if (e.key === 'Escape') onclose();
	}
</script>

<svelte:window onkeydown={onkey} />

<div
	class="overlay"
	role="presentation"
	onclick={(e) => {
		if (e.target === e.currentTarget) onclose();
	}}
>
	<div
		class="sheet"
		class:resizing
		role="dialog"
		aria-modal="true"
		aria-label={title}
		style={width != null ? `--sheet-w: ${width}px` : undefined}
	>
		<div class="sheet-head">
			<span class="sheet-title truncate">{title}</span>
			<div class="spacer"></div>
			<IconButton icon="x" label="Close" onclick={onclose} />
		</div>
		<div class="sheet-body">
			{@render body()}
		</div>
		{#if footer}
			<div class="sheet-foot">{@render footer()}</div>
		{/if}
		{#if resizeKey}
			<!-- Drag the right edge to resize horizontally (desktop only). -->
			<div
				class="sheet-resize"
				role="separator"
				aria-label="Resize dialog width"
				aria-orientation="vertical"
				onpointerdown={startResize}
				onpointermove={onResize}
				onpointerup={endResize}
				onpointercancel={endResize}
			></div>
		{/if}
	</div>
</div>

<style>
	/* When a persisted width is set, the sheet uses it (capped to the viewport).
	   Overrides app.css's max-width: 34rem. Resizing only matters on desktop where
	   the sheet isn't already full-width. */
	@media (min-width: 640px) {
		.sheet {
			width: var(--sheet-w, 34rem);
			max-width: min(var(--sheet-w, 34rem), calc(100vw - 2rem));
		}
	}
	.sheet {
		position: relative;
	}
	.sheet.resizing {
		user-select: none;
	}
	.sheet-resize {
		display: none;
	}
	@media (min-width: 640px) {
		.sheet-resize {
			display: block;
			position: absolute;
			top: 0;
			bottom: 0;
			right: 0;
			width: 12px;
			margin-right: -6px;
			cursor: ew-resize;
			touch-action: none;
			z-index: 2;
		}
		.sheet-resize::after {
			content: '';
			position: absolute;
			top: 50%;
			right: 6px;
			transform: translateY(-50%);
			width: 3px;
			height: 28px;
			border-radius: 999px;
			background: var(--border-strong);
			transition: background 0.12s var(--ease);
		}
		.sheet-resize:hover::after,
		.sheet.resizing .sheet-resize::after {
			background: var(--accent);
		}
	}
</style>
