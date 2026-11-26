<script lang="ts">
	import type { Snippet } from 'svelte';
	import { IconButton, resizeHandle } from '@dorsk/tsumikit';
	import { tick } from 'svelte';
	import { settings, type SpawnDockSide } from '#lib/settings.svelte.ts';
	import { DOCK_MIN_PX, maxDockWidth } from '#lib/dock.ts';
	import { expandSpawnDock, minifySpawnDock } from '#lib/spawnDock.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';

	let {
		side,
		stacked = false,
		width,
		minified = false,
		offset = null,
		body,
		footer
	}: {
		side: SpawnDockSide;
		stacked?: boolean;
		width: string;
		/** Show the rail; the form stays mounted underneath so its draft survives. */
		minified?: boolean;
		/** Distance from the edge when the stats panel shares it. */
		offset?: string | null;
		body: Snippet;
		footer: Snippet;
	} = $props();

	const bodyId = $props.id();
	let dockEl = $state<HTMLElement>();
	const towardEdge = $derived(side === 'left' ? '«' : '»');
	const awayFromEdge = $derived(side === 'left' ? '»' : '«');

	async function toggle(expand: boolean) {
		if (expand) expandSpawnDock();
		else minifySpawnDock();
		await tick();
		dockEl?.querySelector<HTMLElement>('[data-dock-toggle]')?.focus();
	}

	let dragging = $state(false);
	let viewportWidth = $state(0);
	const maxPx = $derived(maxDockWidth(viewportWidth));
</script>

<svelte:window bind:innerWidth={viewportWidth} />

<aside
	bind:this={dockEl}
	class="dock"
	class:dock-left={side === 'left'}
	class:stacked
	class:minified
	aria-label={m.spawn_modal_title()}
	style:--spawn-dock-w={width}
	style:--spawn-dock-offset={offset ?? undefined}
>
	{#if minified}
		<div class="rail">
			<IconButton
				emoji={awayFromEdge}
				variant="ghost"
				label={m.spawn_dock_expand()}
				aria-expanded="false"
				aria-controls={bodyId}
				data-dock-toggle
				onclick={() => toggle(true)}
			/>
		</div>
	{:else}
		<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
		<div
			class="grip"
			class:grip-left={side === 'left'}
			class:dragging
			role="separator"
			tabindex="0"
			aria-orientation="vertical"
			aria-valuemin={DOCK_MIN_PX}
			aria-valuemax={maxPx}
			aria-label={m.dock_resize_grip()}
			title={m.dock_resize_grip()}
			use:resizeHandle={{
				side,
				min: DOCK_MIN_PX,
				max: maxPx,
				onwidth: (px) => settings.setSpawnDock({ width: px }),
				onreset: () => settings.setSpawnDock({ width: undefined }),
				onactive: (a) => {
					dragging = a;
					document.body.classList.toggle('dock-resizing', a);
				}
			}}
		></div>
		<div class="dock-head">
			<span>{m.spawn_modal_title()}</span>
			<IconButton
				emoji={towardEdge}
				variant="ghost"
				label={m.spawn_dock_minify()}
				aria-expanded="true"
				aria-controls={bodyId}
				data-dock-toggle
				onclick={() => toggle(false)}
			/>
		</div>
	{/if}
	<div class="dock-body" id={bodyId} hidden={minified} inert={minified}>
		{@render body()}
	</div>
	<div class="dock-foot" hidden={minified} inert={minified}>{@render footer()}</div>
</aside>

<style>
	/* Pinned to one edge between the header and the bottom nav, scrolling its
	   body on its own. The layout reserves the same width. */
	.dock {
		position: fixed;
		top: calc(var(--header-h) + var(--safe-top));
		bottom: var(--bottom-chrome, calc(var(--nav-h) + var(--safe-bottom)));
		right: 0;
		width: var(--spawn-dock-w);
		display: flex;
		flex-direction: column;
		background: var(--bg-elevated);
		border-left: 1px solid var(--border);
		z-index: 4;
	}
	.dock.minified {
		right: var(--spawn-dock-offset, 0);
	}
	.dock.dock-left {
		right: auto;
		left: 0;
		border-left: 0;
		border-right: 1px solid var(--border);
	}
	.dock.dock-left.minified {
		left: var(--spawn-dock-offset, 0);
	}
	.dock.stacked {
		bottom: 50%;
	}
	.rail {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: var(--sp-2);
		padding: var(--sp-2) 0;
	}
	.dock-body[hidden],
	.dock-foot[hidden] {
		display: none;
	}
	/* A 10px hit area straddling the panel's border, with a 2px line that only
	   shows on hover, focus or while dragging. */
	.grip {
		position: absolute;
		top: 0;
		bottom: 0;
		left: -5px;
		width: 10px;
		cursor: ew-resize;
		touch-action: none;
		z-index: 1;
	}
	.grip-left {
		left: auto;
		right: -5px;
	}
	.grip::before {
		content: '';
		position: absolute;
		top: 50%;
		left: 3px;
		width: 4px;
		height: 2.5rem;
		margin-top: -1.25rem;
		border-radius: var(--r-pill);
		background: var(--border-strong);
	}
	.grip:hover::before,
	.grip.dragging::before {
		background: var(--accent);
	}
	.grip::after {
		content: '';
		position: absolute;
		top: 0;
		bottom: 0;
		left: 4px;
		width: 2px;
		background: var(--accent);
		opacity: 0;
		transition: opacity 0.12s var(--ease);
	}
	.grip:hover::after,
	.grip:focus-visible::after,
	.grip.dragging::after {
		opacity: 1;
	}
	.grip:focus-visible {
		outline: none;
	}
	@media (hover: none) {
		.grip::after {
			opacity: 0.35;
		}
	}
	.dock-head {
		flex: none;
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
		padding: var(--sp-3) var(--sp-3) var(--sp-2);
		font-weight: var(--fw-semibold);
		border-bottom: 1px solid var(--border);
	}
	.dock-body {
		flex: 1;
		min-height: 0;
		overflow-y: auto;
		overflow-x: hidden;
		padding: var(--sp-3);
	}
	.dock-foot {
		flex: none;
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
		border-top: 1px solid var(--border);
	}
</style>
