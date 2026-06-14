<script lang="ts">
	// Conversation drawer header (CCT-301 #6/#7 / CCT-303), extracted from
	// ConversationDrawer. Owns the title + rename, the secondary-action group
	// (font size · rename · copy link · copy markdown · export · fork) which
	// collapses into a ⋯ flyout on mobile, the interrupt/archive controls, and the
	// meta row (status badge, in-place codex model editor or the claude "fork to
	// change model" chip, machine badge, cwd, token usage). Action side-effects
	// are delegated to callbacks; the editing UI state lives here.
	import type { SessionListItem } from '@bindings/SessionListItem';
	import type { Label } from '@bindings/Label';
	import { statusBadgeClass } from '$lib/format';
	import { fontScale, SCALE_LEVELS } from '$lib/fontscale.svelte';
	import AdapterIcon from '$lib/components/atoms/AdapterIcon.svelte';
	import MachineBadge from '$lib/components/molecules/MachineBadge.svelte';
	import LabelBadge from '$lib/components/molecules/LabelBadge.svelte';
	import WorkingDir from '$lib/components/molecules/WorkingDir.svelte';
	import TokenUsage from '$lib/components/molecules/TokenUsage.svelte';
	import { Badge, IconButton, Input, Select, SelectButton, Text } from '@dorsk/tsumikit';

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
		onarchive,
		onTogglePin,
		// Label editing (CCT-360): same picker as the session card — when
		// `onAttachLabel` is supplied the strip is interactive, else read-only.
		allLabels = [],
		onCreateLabel,
		onAttachLabel,
		onDetachLabel,
		onUpdateLabel,
		onDeleteLabel
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
		onTogglePin?: (s: SessionListItem) => void;
		allLabels?: Label[];
		onCreateLabel?: (name: string, color: string) => Promise<Label>;
		onAttachLabel?: (id: string, labelId: string) => void | Promise<void>;
		onDetachLabel?: (id: string, labelId: string) => void | Promise<void>;
		onUpdateLabel?: (labelId: string, patch: { name?: string; color?: string }) => Promise<Label>;
		onDeleteLabel?: (labelId: string) => void | Promise<void>;
	} = $props();

	// Label picker is interactive only when an attach handler is wired in.
	const labelEditable = $derived(!!onAttachLabel);

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
		<IconButton class="tapbtn back" icon="back"  label="Back" onclick={onclose} />
		<!-- Lead group: star · dot · machine · title · labels. Wraps so the label
		     strip falls to its own row when the header is narrow, while the title
		     stays visible and the trailing controls stay pinned on the first line
		     (the title shrink-wraps with flex:0 1 auto; labels are last so they wrap
		     first). The tag picker trigger therefore always trails the title, never
		     mingles with the action buttons. -->
		<span class="lead">
			{#if onTogglePin}
				<span
					class="star"
					class:on={session.pinned}
					role="button"
					tabindex="0"
					title={session.pinned ? 'Unpin' : 'Pin to top (exempt from auto-archive)'}
					aria-pressed={session.pinned}
					aria-label={session.pinned ? 'Unpin session' : 'Pin session'}
					onclick={() => onTogglePin?.(session)}
					onkeydown={(e: KeyboardEvent) => {
						if (e.key === 'Enter' || e.key === ' ') {
							e.preventDefault();
							onTogglePin?.(session);
						}
					}}>{session.pinned ? '★' : '☆'}</span
				>
			{/if}
			<span class="dot {livenessClass}" title={session.hibernated ? 'hibernated' : session.liveness}></span>
			<MachineBadge name={session.machine_name} id={session.machine_id} hue={session.machine_hue} mono />
			<div class="dtitle" class:editing={renaming}>
				{#if renaming}
					<Input bind:value={newName} onkeydown={(e: KeyboardEvent) => e.key === 'Enter' && doRename()} />
				{:else}
					<Text class="name" weight="semibold" size="md" truncate>{headTitle}</Text>
				{/if}
			</div>
			{#if session.labels.length > 0 || labelEditable}
				<LabelBadge
					labels={session.labels}
					editable={labelEditable}
					{allLabels}
					onCreate={onCreateLabel}
					onAttach={(lid) => onAttachLabel?.(session.id, lid)}
					onDetach={(lid) => onDetachLabel?.(session.id, lid)}
					onUpdate={onUpdateLabel}
					onDelete={onDeleteLabel}
				/>
			{/if}
		</span>
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
			<IconButton class="tapbtn" icon="check"  label="Save" onclick={doRename} />
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
			<IconButton class="tapbtn interrupt" icon="stop"  label="Interrupt turn" title="Interrupt the in-flight turn" onclick={oninterrupt} />
			<IconButton class="tapbtn archive" icon="archive"  label="Archive" onclick={onarchive} />
		{/if}
	</div>
	<div class="hmeta row row-wrap">
		{#if showStatusBadge}<Badge class={statusBadgeClass(session.status)}>{session.status}</Badge>{/if}
		<WorkingDir path={session.working_dir} />
		<div class="meta-trail">
		<TokenUsage usage={session.token_usage} />
		{#if isCodexSession && !archived}
			{#if modelEditing}
				<!-- display:contents wrapper exists only to give the compact Selects a
				     real scoped ancestor (.model-edit), so their width/fill reach-in
				     isn't a document-wide :global leak. -->
				<span class="model-edit">
					<Badge class="row" style="gap:var(--sp-1);padding:0.05rem var(--sp-1)">
						<Select compact chevron={false} bind:value={pendingModel} aria-label="Model">
							{#each codexModels as m (m.v)}<option value={m.v}>{m.label}</option>{/each}
						</Select>
						<Select compact chevron={false} bind:value={pendingEffort} aria-label="Effort">
							{#each codexEfforts as e (e)}<option value={e}>{e || 'default effort'}</option>{/each}
						</Select>
						<IconButton class="tapbtn" icon="check"  label="Apply" onclick={applyModelChange} />
						<IconButton class="tapbtn" icon="x"  label="Cancel" onclick={() => (modelEditing = false)} />
					</Badge>
				</span>
			{:else}
				<Badge
					as="button"
					mono
					title="Change model / effort for the next turn"
					onclick={openModelEditor}
				>{session.model ?? 'default'}{session.effort ? ` · ${session.effort}` : ''} ✎</Badge>
			{/if}
		{:else if session.model || session.effort}
			<Badge
				as="button"
				mono
				title="Claude can't switch model in place — fork to change model"
				onclick={onfork}
			>{session.model ?? ''}{session.effort ? ` · ${session.effort}` : ''} ⑂</Badge>
		{/if}
		<AdapterIcon adapter={session.adapter_id} size={20} />
		</div>
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
		/* Collapse the secondary actions based on the DRAWER's own width, not the
		   viewport (CCT-301 #7): a narrow-but-on-desktop drawer should fold its
		   buttons into the ⋯ flyout so the title stays visible. The header is the
		   size container the rules below query against. */
		container: drawer-head / inline-size;
	}
	.hrow {
		display: flex;
		align-items: flex-start;
		gap: var(--sp-2);
		position: relative;
	}
	/* Secondary actions: inline on desktop, ⋯ flyout on mobile (CCT-301 #7). */
	.secondary {
		display: contents;
	}
	/* Desktop shows every action inline, so the ⋯ flyout toggle is pointless
	   there — only surface it when actions actually collapse. */
	/* NB: `.more` is rendered by the IconButton child component, so the rule
	   MUST be `:global` — a plain `.more` selector is scoped to THIS
	   component and never matches the child <button>, which is why the kebab
	   would otherwise show on desktop. */
	.dhead :global(.tapbtn.more) {
		display: none;
	}
	@container drawer-head (max-width: 640px) {
		.dhead :global(.tapbtn.more) {
			display: inline-flex;
		}
		.secondary {
			display: none;
			position: absolute;
			top: calc(100% + var(--sp-1));
			right: 0;
			/* Stack above the message list + composer so chat content can't sit on
			   top of the flyout. */
			z-index: 60;
			flex-direction: column;
			align-items: stretch;
			/* Fixed, content-comfortable width: the rows are width:100%, so a
			   max-content panel width would be circular and collapse to min-width,
			   overflowing the long labels off the right edge. Pin a width that fits
			   the labels and never exceeds the viewport. */
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
		/* The `.dhead` prefix is required: it raises specificity above the base
		   `.dhead :global(.tapbtn)` rule below (equal specificity, but that rule is
		   later in source), so without it these flyout overrides never apply. */
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
	/* Lead group wraps (label strip drops to its own row when narrow) while the
	   trailing action buttons stay pinned to the first line. */
	.lead {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		column-gap: var(--sp-2);
		row-gap: var(--sp-1);
		flex: 1 1 auto;
		min-width: 0;
		/* One button-height tall so a single-line header reads centered against the
		   back/action buttons (hrow is flex-start); when the label strip wraps the
		   lead grows downward and the buttons stay aligned to the title line. */
		min-height: 2.5rem;
	}
	/* Title shrink-wraps so labels can sit beside it and wrap first; it never
	   eats the whole row (which would force labels to wrap even with space). */
	.dtitle {
		flex: 0 1 auto;
		min-width: 0;
	}
	/* While renaming, let the input claim the lead's free width. */
	.dtitle.editing {
		flex: 1 1 12rem;
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
		align-items: center;
	}
	/* Push token usage · model · logo to the right edge, opposite the working
	   dir, mirroring the session card footer. */
	.meta-trail {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		flex: none;
		margin-left: auto;
	}
	/* Star/pin toggle in the lead row (mirrors SessionCard). */
	.star {
		background: none;
		border: none;
		cursor: pointer;
		user-select: none;
		padding: 0;
		line-height: 1;
		font-size: 1.35rem;
		color: var(--text-faint);
		flex: none;
	}
	.star.on,
	.star:hover {
		color: var(--warn, #e0a800);
	}
	.model-edit {
		display: contents;
	}
	/* tsumikit Select `compact` gives the dense padding/font and `chevron={false}`
	   drops the chevron; only the inline auto-width + lighter fill remain, reached
	   in scoped under .model-edit so the selectors can't leak. */
	.model-edit :global(.select-wrap) {
		width: auto;
	}
	.model-edit :global(.select) {
		width: auto;
		background: var(--bg-elevated-2);
		border-color: var(--border);
		border-radius: var(--r-sm, 4px);
	}
</style>
