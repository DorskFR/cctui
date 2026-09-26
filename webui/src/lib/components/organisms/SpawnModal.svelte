<script lang="ts">
	import { AutoGrid, Button, Callout, Dropzone, Modal, resizeHandle } from '@dorsk/tsumikit';
	import { isSubmitChord } from '$lib/platform';
	import { dialogBackdropGuard } from '$lib/dialogBackdropGuard';
	import { settings, type SpawnDockSide } from '$lib/settings.svelte';
	import { SPAWN_DOCK_WIDTH } from '$lib/spawnDock.svelte';
	import { DOCK_MIN_PX, maxDockWidth } from '$lib/dock';
	import { m } from '$lib/paraglide/messages';
	import type { SpawnPrefill } from './spawn/types';
	import { SpawnForm } from './spawn/spawnForm.svelte';
	import SpawnTargetSection from './spawn/SpawnTargetSection.svelte';
	import SpawnPromptSection from './spawn/SpawnPromptSection.svelte';
	import SpawnGrantsSection from './spawn/SpawnGrantsSection.svelte';

	let dragging = $state(false);
	let viewportWidth = $state(0);
	const maxPx = $derived(maxDockWidth(viewportWidth));

	let {
		onclose,
		onspawned,
		prefill = null,
		docked = null,
		stacked = false,
		dockWidth = SPAWN_DOCK_WIDTH,
		autosaveDelay = 2000
	}: {
		// Modal: close the dialog. Docked: the form is done with (spawned, saved
		// as draft, or cleared) — the parent remounts it so it reseeds exactly the
		// way a reopened modal would.
		onclose: () => void;
		onspawned: () => void;
		// Docked panel (Settings › New session): the same form pinned to one edge
		// of the Sessions screen instead of inside a Modal.
		docked?: SpawnDockSide | null;
		// Docked and sharing its column with the stats panel: top half only.
		stacked?: boolean;
		// Docked: the width the layout reserved on that edge (resolveDocks).
		dockWidth?: string;
		// "New session from same script" / "Edit draft": seed the form from a
		// session's config or a draft row. Non-empty prefill values win over the
		// target's local slot; empty ones never clear what the slot holds.
		prefill?: SpawnPrefill | null;
		// Quiet time before the form is mirrored to its server draft.
		autosaveDelay?: number;
	} = $props();

	// svelte-ignore state_referenced_locally
	const sf = new SpawnForm({
		onclose,
		onspawned,
		prefill,
		autosaveDelay: () => autosaveDelay,
		docked: () => !!docked
	});

	/** Whether the form holds anything the user would miss. */
	export function isDirty(): boolean {
		return sf.isDirty();
	}
	/** The server draft this form mirrors, once autosaved. */
	export function currentDraftId(): string | null {
		return sf.draftId;
	}
	/** Write the form to its draft row now; false when it can't be a draft yet. */
	export function flushDraft(): Promise<boolean> {
		return sf.flushDraft();
	}
</script>

<!-- The form body and the action row are snippets so the Modal and the docked
     panel render the very same markup; only the chrome around them differs. -->
<svelte:window bind:innerWidth={viewportWidth} />

{#if docked}
	<aside
		class="dock"
		class:dock-left={docked === 'left'}
		class:stacked
		aria-label={m.spawn_modal_title()}
		style:--spawn-dock-w={dockWidth}
	>
	<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
	<div
		class="grip"
		class:grip-left={docked === 'left'}
		class:dragging
		role="separator"
		tabindex="0"
		aria-orientation="vertical"
		aria-valuemin={DOCK_MIN_PX}
		aria-valuemax={maxPx}
		aria-label={m.dock_resize_grip()}
		title={m.dock_resize_grip()}
		use:resizeHandle={{
			side: docked,
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
		</div>
		<div class="dock-body">{@render body()}</div>
		<div class="dock-foot">{@render footer()}</div>
	</aside>
{:else}
	<Modal title={m.spawn_modal_title()} {onclose} resizeKey="cctui_spawn_modal_width" {body} {footer} />
{/if}

{#snippet body()}
	<!-- The whole dialog is a file drop area (machine target only). -->
	<Dropzone
		overlay
		multiple
		label={m.spawn_dropzone_label()}
		disabled={sf.target !== 'machine'}
		onfiles={sf.addFiles}
	>
		<div
			class="stack"
			data-journey="spawn"
			use:dialogBackdropGuard
			onkeydown={(e: KeyboardEvent) => {
				if (isSubmitChord(e) && !sf.busy && sf.valid) {
					e.preventDefault();
					void sf.submit();
				}
			}}
		>
			<SpawnTargetSection {sf} />
			<SpawnPromptSection {sf} />
			<SpawnGrantsSection {sf} />
		</div>
	</Dropzone>
{/snippet}
{#snippet footer()}
	{#if sf.spawnFailure}
		<Callout tone="danger" title={m.spawn_failure_inline()} style="flex-basis:100%">
			<pre class="spawn-failure-detail">{sf.spawnFailure}</pre>
		</Callout>
	{/if}
	<span class="foot-secondary">
		<Button onclick={sf.clearForm}>{m.spawn_clear()}</Button>
		{#if sf.target === 'machine'}
			<Button
				data-journey="draft"
				disabled={sf.busy || !sf.draftValid}
				title={sf.disabledReason}
				onclick={sf.submitDraft}
			>
				{m.spawn_draft()}
			</Button>
		{/if}
	</span>
	<span class="foot-primary">
		<Button
			data-journey="submit"
			variant="primary"
			grow
			loading={sf.busy}
			disabled={sf.busy || !sf.valid}
			title={sf.disabledReason}
			onclick={sf.submit}
		>
			{sf.spawnLabel}
		</Button>
	</span>
{/snippet}

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.spawn-failure-detail {
		margin: var(--sp-1) 0 0;
		max-height: 8rem;
		overflow: auto;
		white-space: pre-wrap;
		font-family: var(--font-mono);
		font-size: var(--text-xs);
	}
	/* Docked panel: pinned to one edge between the header and the bottom nav,
	   scrolling its body on its own. The layout reserves the same width. */
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
	.dock.dock-left {
		right: auto;
		left: 0;
		border-left: 0;
		border-right: 1px solid var(--border);
	}
	.dock.stacked {
		bottom: 50%;
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
	/* An always-visible knob in the middle of the edge, like the kit's panel handle. */
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
	.foot-secondary {
		display: flex;
		gap: var(--sp-2);
		flex: 0 1 auto;
	}
	/* The basis is the width below which the primary action would be squeezed:
	   past it the row wraps and grow makes it span the whole footer. */
	.foot-primary {
		display: flex;
		flex: 1 1 11rem;
	}
</style>
