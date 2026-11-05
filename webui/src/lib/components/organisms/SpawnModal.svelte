<script lang="ts">
	import { Dropzone, Modal } from '@dorsk/tsumikit';
	import { submitChord } from '$lib/platform';
	import { dialogBackdropGuard } from '$lib/dialogBackdropGuard';
	import type { SpawnDockSide } from '$lib/settings.svelte';
	import { SPAWN_DOCK_WIDTH } from '$lib/spawnDock.svelte';
	import { m } from '$lib/paraglide/messages';
	import type { SpawnPrefill } from './spawn/types';
	import { SpawnForm } from './spawn/spawnForm.svelte';
	import SpawnDock from './spawn/SpawnDock.svelte';
	import SpawnFooter from './spawn/SpawnFooter.svelte';
	import SpawnTargetSection from './spawn/SpawnTargetSection.svelte';
	import SpawnPromptSection from './spawn/SpawnPromptSection.svelte';
	import SpawnGrantsSection from './spawn/SpawnGrantsSection.svelte';

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
		onspawned: (sessionId: string | null) => void;
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
{#if docked}
	<SpawnDock side={docked} {stacked} width={dockWidth} {body} {footer} />
{:else}
	<Modal
		title={m.spawn_modal_title()}
		{onclose}
		resizeKey="cctui_spawn_modal_width"
		footerFill
		{body}
		{footer}
	/>
{/if}

{#snippet body()}
	<!-- The whole dialog is a file drop area (machine target only). -->
	<Dropzone
		overlay
		multiple
		label={m.spawn_dropzone_label()}
		disabled={sf.target !== 'machine'}
		onfiles={sf.att.add}
		onactive={sf.att.setDragActive}
	>
		<div
			class="stack"
			data-journey="spawn"
			use:dialogBackdropGuard
			use:submitChord={() => {
				if (sf.busy || !sf.valid) return false;
				void sf.submit();
				return true;
			}}
		>
			<SpawnTargetSection {sf} />
			<SpawnPromptSection {sf} />
			<SpawnGrantsSection {sf} />
		</div>
	</Dropzone>
{/snippet}
{#snippet footer()}
	<SpawnFooter {sf} />
{/snippet}

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
</style>
