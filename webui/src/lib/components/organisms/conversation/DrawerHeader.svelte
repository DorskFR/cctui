<script lang="ts">
	// Conversation drawer header (CCT-301 #6/#7 / CCT-303), extracted from
	// ConversationDrawer. Owns the title + rename, the secondary-action group
	// (font size · rename · copy link · copy markdown · export · fork) which
	// collapses into a ⋯ flyout on mobile, the interrupt/archive controls, and the
	// meta row (status badge, in-place codex model editor or the claude "fork to
	// change model" chip, machine badge, cwd, token usage). Action side-effects
	// are delegated to callbacks; the editing UI state lives here.
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { statusBadgeClass } from '$lib/format';
	import { toasts } from '$lib/toast.svelte';
	import { fontScale, SCALE_LEVELS } from '$lib/fontscale.svelte';
	import AdapterIcon from '$lib/components/atoms/AdapterIcon.svelte';
	import MachineBadge from '$lib/components/molecules/MachineBadge.svelte';
	import TokenUsage from '$lib/components/molecules/TokenUsage.svelte';
	import IconButton from '$lib/components/molecules/IconButton.svelte';
	import Badge from '$lib/components/atoms/Badge.svelte';
	import Chip from '$lib/components/atoms/Chip.svelte';
	import Input from '$lib/components/atoms/Input.svelte';
	import SelectButton from '$lib/components/molecules/SelectButton.svelte';

	let {
		session,
		archived,
		isCodexSession,
		livenessClass,
		showStatusBadge,
		onclose,
		onrename,
		onsetmodel,
		oncopylink,
		oncopymarkdown,
		onexport,
		onfork,
		oninterrupt,
		onarchive
	}: {
		session: SessionListItem;
		archived: boolean;
		isCodexSession: boolean;
		livenessClass: string;
		showStatusBadge: boolean;
		onclose: () => void;
		onrename: (name: string) => void;
		onsetmodel: (model: string, effort: string) => void;
		oncopylink: () => void;
		oncopymarkdown: () => void;
		onexport: () => void;
		onfork: () => void;
		oninterrupt: () => void;
		onarchive: () => void;
	} = $props();

	const headTitle = $derived(session.name || session.working_dir);

	const codexEfforts = ['', 'low', 'medium', 'high', 'xhigh'];
	const codexModels = [
		{ v: '', label: 'Default' },
		{ v: 'gpt-5.5-codex', label: 'GPT-5.5 Codex' },
		{ v: 'gpt-5.4-codex', label: 'GPT-5.4 Codex' }
	];

	let renaming = $state(false);
	let newName = $state(session.name ?? '');
	// Mobile header overflow menu (CCT-301 #7): on narrow screens only Stop +
	// Archive stay inline; the rest collapse into a "⋯" flyout. Kept open while
	// renaming so the ✓ save button is reachable.
	let moreOpen = $state(false);
	// In-place model/effort editor (CCT-303), codex only.
	let modelEditing = $state(false);
	let pendingModel = $state('');
	let pendingEffort = $state('');

	function doRename() {
		const n = newName.trim();
		renaming = false;
		if (!n) return;
		onrename(n);
	}
	function openModelEditor() {
		pendingModel = session.model ?? '';
		pendingEffort = session.effort ?? '';
		modelEditing = true;
	}
	function applyModelChange() {
		const model = pendingModel.trim();
		const effort = pendingEffort.trim();
		modelEditing = false;
		if (!model && !effort) return;
		onsetmodel(model, effort);
	}

	async function copyText(text: string) {
		try {
			await navigator.clipboard.writeText(text);
			toasts.ok('Copied');
		} catch {
			toasts.err('Clipboard unavailable');
		}
	}

	function closeMoreFromOutside(e: PointerEvent) {
		if (!moreOpen) return;
		const t = e.target as HTMLElement | null;
		if (t?.closest('.secondary') || t?.closest('.more')) return;
		moreOpen = false;
	}
	function onWinKey(e: KeyboardEvent) {
		if (e.key !== 'Escape' || renaming) return;
		if (moreOpen) moreOpen = false;
		else onclose();
	}
</script>

<svelte:window onkeydown={onWinKey} onpointerdown={closeMoreFromOutside} />

<div class="dhead">
	<div class="hrow">
		<IconButton class="tapbtn back" icon="back" label="Back" onclick={onclose} />
		<AdapterIcon adapter={session.adapter_id} size={20} />
		<span class="dot {livenessClass}" title={session.hibernated ? 'hibernated' : session.liveness}></span>
		<div class="dtitle">
			{#if renaming}
				<Input bind:value={newName} onkeydown={(e: KeyboardEvent) => e.key === 'Enter' && doRename()} />
			{:else}
				<span class="truncate name">{headTitle}</span>
			{/if}
		</div>
		<!-- Secondary actions (CCT-301 #7): inline on desktop, collapsed into the
		     ⋯ flyout on mobile so a long title + many buttons no longer overflow.
		     Font-size is the left-most action; a single fork lives at the end of
		     the group (CCT-345). -->
		<div class="secondary" class:open={moreOpen || renaming}>
		<!-- UI font size (CCT-301 #6): the SAME discrete "A" control as the main
		     window header (CCT-297 #11), promoted out of the formatting bar up to
		     this top-level row so scaling is reachable without scanning the
		     JSON/Diff/Tables toggles. Both write the single global fontScale.
		     `font-pick` is kept as a hook for the mobile flyout flattening. -->
		<SelectButton
			class="font-pick"
			glyph="A"
			label="Font size"
			title="UI font size"
			value={fontScale.levelId}
			options={SCALE_LEVELS.map((l) => ({ value: l.id, label: l.label }))}
			onchange={(v) => fontScale.set(v)}
		/>
		{#if renaming}
			<IconButton class="tapbtn" icon="check" label="Save" onclick={doRename} />
		{:else}
			<IconButton
				class="tapbtn"
				icon="edit"
				label="Rename"
				onclick={() => {
					renaming = true;
					newName = session.name ?? '';
				}}
			/>
		{/if}
		<IconButton
			class="tapbtn"
			icon="link"
			label="Copy shareable link"
			title="Copy a stable link to this session (paste in a PR — login-gated)"
			onclick={oncopylink}
		/>
		<IconButton
			class="tapbtn"
			icon="markdown"
			label="Copy conversation as Markdown"
			title="Copy the whole conversation as Markdown (honors the view filters)"
			onclick={oncopymarkdown}
		/>
		<IconButton
			class="tapbtn"
			icon="download"
			label="Export conversation"
			title="Download transcript as HTML (print it for a PDF)"
			onclick={onexport}
		/>
		<IconButton
			class="tapbtn fork-action"
			icon="fork"
			label="Fork conversation"
			title="Fork into a new conversation (optionally change model)"
			onclick={onfork}
		/>
		</div>
		<!-- Mobile-only overflow toggle (CCT-301 #7); hidden on desktop. -->
		<IconButton
			class="tapbtn more"
			icon="more"
			label="More actions"
			aria-expanded={moreOpen}
			title="More actions"
			onclick={() => (moreOpen = !moreOpen)}
		/>
		{#if !archived}
			<IconButton class="tapbtn interrupt" icon="stop" label="Interrupt turn" title="Interrupt the in-flight turn" onclick={oninterrupt} />
			<IconButton class="tapbtn archive" icon="archive" label="Archive" onclick={onarchive} />
		{/if}
	</div>
	<div class="hmeta row row-wrap">
		{#if showStatusBadge}<Badge class={statusBadgeClass(session.status)}>{session.status}</Badge>{/if}
		{#if isCodexSession && !archived}
			{#if modelEditing}
				<Chip class="row" style="gap:var(--sp-1);padding:0.05rem var(--sp-1)">
					<select class="mini-select" bind:value={pendingModel} aria-label="Model">
						{#each codexModels as m (m.v)}<option value={m.v}>{m.label}</option>{/each}
					</select>
					<select class="mini-select" bind:value={pendingEffort} aria-label="Effort">
						{#each codexEfforts as e (e)}<option value={e}>{e || 'default effort'}</option>{/each}
					</select>
					<IconButton class="tapbtn" icon="check" label="Apply" onclick={applyModelChange} />
					<IconButton class="tapbtn" icon="x" label="Cancel" onclick={() => (modelEditing = false)} />
				</Chip>
			{:else}
				<Chip
					as="button"
					mono
					title="Change model / effort for the next turn"
					onclick={openModelEditor}
				>{session.model ?? 'default'}{session.effort ? ` · ${session.effort}` : ''} ✎</Chip>
			{/if}
		{:else if session.model || session.effort}
			<Chip
				as="button"
				mono
				title="Claude can't switch model in place — fork to change model"
				onclick={onfork}
			>{session.model ?? ''}{session.effort ? ` · ${session.effort}` : ''} ⑂</Chip>
		{/if}
		<MachineBadge name={session.machine_name} id={session.machine_id} hue={session.machine_hue} mono />
		<Chip
			as="button"
			mono
			class="truncate"
			style="flex:1;min-width:6rem;text-align:left"
			title="Click to copy — {session.working_dir}"
			onclick={() => copyText(session.working_dir)}
		>📁 {session.working_dir} ⧉</Chip>
		<TokenUsage usage={session.token_usage} />
	</div>
</div>

<style>
	.dhead {
		position: sticky;
		top: 0;
		z-index: 2;
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
		border-bottom: 1px solid var(--border);
		background: var(--bg-elevated);
	}
	.hrow {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		position: relative;
	}
	/* Secondary actions: inline on desktop, ⋯ flyout on mobile (CCT-301 #7). */
	.secondary {
		display: contents;
	}
	/* Desktop shows every action inline, so the ⋯ flyout toggle is pointless
	   there — only surface it when actions actually collapse (CCT-345). */
	/* NB: `.more` is rendered by the IconButton child component, so the rule
	   MUST be `:global` — a plain `.more` selector is scoped to THIS
	   component and never matches the child <button>, which is why the kebab
	   leaked onto desktop (CCT-323). */
	.dhead :global(.tapbtn.more) {
		display: none;
	}
	@media (max-width: 959px) {
		.dhead :global(.tapbtn.more) {
			display: inline-flex;
		}
		.secondary {
			display: none;
			position: absolute;
			top: calc(100% + var(--sp-1));
			right: 0;
			/* Above the message list + composer; the old z:5 let chat content sit on
			   top of the flyout (CCT-345). */
			z-index: 60;
			flex-direction: column;
			align-items: stretch;
			/* Fixed, content-comfortable width: the rows are width:100%, which made a
			   max-content panel width circular so it collapsed to min-width and the
			   long labels overflowed off the right edge (CCT-345). Pin a width that
			   fits the labels and never exceeds the viewport. */
			width: 17rem;
			max-width: calc(100vw - 1.5rem);
			gap: var(--sp-1);
			padding: var(--sp-2);
			background: var(--bg-elevated-2);
			border: 1px solid var(--border-strong);
			border-radius: var(--r-md);
			box-shadow: var(--shadow-lg, 0 8px 24px rgba(0, 0, 0, 0.5));
		}
		.secondary.open {
			display: flex;
		}
		/* Flyout rows are icon + text label, NOT the bordered 2.5rem icon-chip
		   used in the desktop toolbar. Reusing the .tapbtn primitive drew an
		   empty bordered square around each icon and broke alignment (CCT-323);
		   here we flatten it into a borderless, auto-height, full-width row. */
		/* NB: the `.dhead` prefix raises specificity ABOVE the base
		   `.dhead :global(.tapbtn)` rule below (equal specificity but later in
		   source) — without it the rows stayed pinned at the 2.5rem icon-chip
		   width and the labels wrapped inside a 40px box (CCT-345). */
		.dhead .secondary :global(.tapbtn),
		.dhead .secondary :global(.font-pick) {
			width: 100%;
			min-width: 0;
			height: auto;
			min-height: 2.25rem;
			justify-content: flex-start;
			gap: var(--sp-2);
			padding: var(--sp-1) var(--sp-2);
			font-size: var(--fs-sm);
			background: none;
			border: none;
			border-radius: var(--r-sm);
		}
		.dhead .secondary :global(.tapbtn):hover,
		.dhead .secondary :global(.font-pick):hover {
			background: var(--bg-elevated-3, var(--bg-elevated-2));
		}
		/* Plain inline icon glyph inside a row — no chip box. */
		.dhead .secondary :global(.tapbtn svg) {
			flex: none;
		}
		.dhead .secondary :global(.tapbtn)::after,
		.dhead .secondary :global(.font-pick)::after {
			content: attr(aria-label);
			font-size: var(--fs-sm);
			font-weight: var(--fw-medium);
			/* Let a long label wrap inside the panel instead of clipping at the
			   viewport edge (CCT-345). */
			white-space: normal;
			text-align: left;
			line-height: 1.2;
		}
	}
	.dtitle {
		flex: 1;
		min-width: 0;
	}
	.name {
		font-weight: var(--fw-semibold);
		font-size: var(--fs-md);
	}
	/* Bigger, easy-to-tap icon buttons with a tinted, outlined chip look. */
	.dhead :global(.tapbtn) {
		flex: none;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 2.5rem;
		min-width: 2.5rem;
		height: 2.5rem;
		min-height: 2.5rem;
		padding: 0;
		font-size: 1.35rem;
		line-height: 1;
		border-radius: var(--r-md);
		background: var(--bg-elevated-2);
		border: 1px solid var(--border-strong);
		color: var(--text);
	}
	.dhead :global(.tapbtn.back) {
		font-size: 1.8rem;
	}
	.dhead :global(.tapbtn.archive) {
		order: 10;
		color: var(--warn);
		border-color: color-mix(in srgb, var(--warn) 40%, var(--border-strong));
		background: color-mix(in srgb, var(--warn) 10%, var(--bg-elevated-2));
	}
	.dhead :global(.tapbtn.interrupt) {
		order: 11;
		color: var(--danger, #bf616a);
		border-color: color-mix(in srgb, var(--danger, #bf616a) 40%, var(--border-strong));
		background: color-mix(in srgb, var(--danger, #bf616a) 10%, var(--bg-elevated-2));
	}
	.hmeta {
		gap: var(--sp-2);
	}
	.mini-select {
		font-size: var(--fs-xs);
		color: var(--text);
		background: var(--bg-elevated-2);
		border: 1px solid var(--border);
		border-radius: var(--r-sm, 4px);
		padding: 0 0.2rem;
	}
</style>
