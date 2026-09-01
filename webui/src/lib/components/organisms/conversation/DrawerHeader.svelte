<script lang="ts">
	// Conversation drawer header. Owns the title + rename, the secondary-action group
	// (font size · rename · copy link · copy markdown · export · fork) which
	// collapses into a ⋯ flyout on mobile, the interrupt/archive controls, and the
	// meta row (status badge, in-place codex model editor or the claude "fork to
	// change model" chip, machine badge, cwd, token usage). Action side-effects
	// are delegated to callbacks; the editing UI state lives here.
	import type { SessionListItem } from '@bindings/SessionListItem';
	import type { Label } from '@bindings/Label';
	import { statusBadgeClass } from '$lib/format';
	import { fontScale, SCALE_LEVELS } from '$lib/fontscale.svelte';
	import { settings } from '$lib/settings.svelte';
	import { isArchiveChord } from '$lib/platform';
	import AdapterIcon from '$lib/components/atoms/AdapterIcon.svelte';
	import MachineBadge from '$lib/components/molecules/MachineBadge.svelte';
	import AccountBadge from '$lib/components/molecules/AccountBadge.svelte';
	import SessionDot from '$lib/components/molecules/SessionDot.svelte';
	import LabelBadge from '$lib/components/molecules/LabelBadge.svelte';
	import WorkingDir from '$lib/components/molecules/WorkingDir.svelte';
	import TokenUsage from '$lib/components/molecules/TokenUsage.svelte';
	import LangfuseChip from '$lib/components/molecules/LangfuseChip.svelte';
	import { Badge, IconButton, Input, Select, SelectButton, Text } from '@dorsk/tsumikit';
	import { codexModelsFor, codexEffortsFor } from '$lib/harnessModels';
	import { useCodexModels } from '$lib/queries';
	import { m } from '$lib/paraglide/messages';

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
		onforkselect,
		forkSelectActive = false,
		oninterrupt,
		onarchive,
		onstoparchive,
		onTogglePin,
		// Opens the at-will account switcher from the key glyph; when omitted
		// the badge stays a read-only indicator.
		onAccountClick,
		// Label editing: same picker as the session card — when
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
		// Toggle multi-select-to-fork mode; omitted → button hidden
		// (codex sessions have no partial-fork primitive).
		onforkselect?: () => void;
		forkSelectActive?: boolean;
		oninterrupt: () => void;
		onarchive: () => void;
		// Stop-then-archive, fired by the ⌘/Ctrl+E keyboard chord.
		onstoparchive: () => void;
		onTogglePin?: (s: SessionListItem) => void;
		onAccountClick?: () => void;
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

	let renaming = $state(false);
	// svelte-ignore state_referenced_locally
	let newName = $state(session.name ?? '');
	// Mobile header overflow menu: on narrow screens only Stop +
	// Archive stay inline; the rest collapse into a "⋯" flyout. Kept open while
	// renaming so the ✓ save button is reachable.
	let moreOpen = $state(false);
	// In-place model/effort editor, codex only.
	let modelEditing = $state(false);
	let pendingModel = $state('');
	let pendingEffort = $state('');

	// Machine-scoped codex catalog: fetched only while the editor is
	// open, offers the account's real models + supported efforts, static fallback.
	const codexCatalog = useCodexModels(() =>
		isCodexSession && modelEditing ? session.machine_id : ''
	);
	const codexModelOptions = $derived(codexModelsFor(codexCatalog.data));
	const codexEffortOptions = $derived(codexEffortsFor(codexCatalog.data, pendingModel));

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
		// Archive chord (⌘ E / Ctrl+E): interrupt any running turn and archive the
		// session, which then dismisses the drawer. Opt-out via Settings. Skipped
		// while renaming (so the chord can't fire mid-edit) and on already-archived
		// sessions (nothing to archive). Window-level so it works regardless of
		// whether focus is in the composer.
		if (!archived && !renaming && settings.archiveShortcut && isArchiveChord(e)) {
			e.preventDefault();
			onstoparchive();
			return;
		}
		if (e.key !== 'Escape' || renaming) return;
		if (moreOpen) moreOpen = false;
		else onclose();
	}
</script>

<svelte:window onkeydown={onWinKey} onpointerdown={closeMoreFromOutside} />

<div class="dhead">
	<div class="hrow">
		<IconButton class="tapbtn back" icon="back"  label={m.drawer_back()} onclick={onclose} />
		{#if onTogglePin}
			<span
				class="star"
				class:on={session.pinned}
				role="button"
				tabindex="0"
				title={session.pinned ? m.drawer_unpin_title() : m.drawer_pin_title()}
				aria-pressed={session.pinned}
				aria-label={session.pinned ? m.drawer_unpin_aria() : m.drawer_pin_aria()}
				onclick={() => onTogglePin?.(session)}
				onkeydown={(e: KeyboardEvent) => {
					if (e.key === 'Enter' || e.key === ' ') {
						e.preventDefault();
						onTogglePin?.(session);
					}
				}}>{session.pinned ? '★' : '☆'}</span
			>
		{/if}
		<SessionDot {session} {livenessClass} />
		<MachineBadge name={session.machine_name} id={session.machine_id} hue={session.machine_hue} mono />
		<AccountBadge name={session.account_name} onclick={onAccountClick} showName={settings.accountNames} />
		<div class="dtitle">
			{#if renaming}
				<Input bind:value={newName} onkeydown={(e: KeyboardEvent) => e.key === 'Enter' && doRename()} />
			{:else}
				<Text class="name" weight="semibold" size="md" truncate>{headTitle}</Text>
				{#if session.labels.length === 0 && labelEditable}
					<!-- No labels yet: the tag picker rides inline right after the title
					     text rather than claiming an empty full-width row. Once labels
					     exist the strip moves to its own row below (see .hlabels). -->
					<LabelBadge
						labels={[]}
						editable
						{allLabels}
						onCreate={onCreateLabel}
						onAttach={(lid) => onAttachLabel?.(session.id, lid)}
						onDetach={(lid) => onDetachLabel?.(session.id, lid)}
						onUpdate={onUpdateLabel}
						onDelete={onDeleteLabel}
					/>
				{/if}
			{/if}
		</div>
		<!-- UI font size: the SAME discrete "A" control as
		     the main window header. It stays a standalone icon button
		     at all widths — it does NOT fold into the ⋯ flyout on mobile, sitting
		     just left of it instead. Both write the single global
		     fontScale. -->
		<SelectButton
			class="font-pick"
			glyph="A"
			label={m.drawer_font_size_label()}
			title={m.drawer_font_size_title()}
			value={fontScale.levelId}
			options={SCALE_LEVELS.map((l) => ({ value: l.id, label: l.label }))}
			onchange={(v) => fontScale.set(v)}
		/>
		<!-- Secondary actions: inline on desktop, collapsed into the
		     ⋯ flyout on mobile so a long title + many buttons no longer overflow.
		     A single fork lives at the end of the group. -->
		<div class="secondary" class:open={moreOpen || renaming}>
		{#if renaming}
			<IconButton class="tapbtn" icon="check"  label={m.common_save()} onclick={doRename} />
		{:else}
			<IconButton
				class="tapbtn"
				icon="edit"

				label={m.drawer_rename()}
				onclick={() => {
					renaming = true;
					newName = session.name ?? '';
				}}
			/>
		{/if}
		<IconButton
			class="tapbtn"
			icon="link"

			label={m.drawer_copy_link_label()}
			title={m.drawer_copy_link_title()}
			onclick={oncopylink}
		/>
		<IconButton
			class="tapbtn"
			icon="markdown"

			label={m.drawer_copy_markdown_label()}
			title={m.drawer_copy_markdown_title()}
			onclick={oncopymarkdown}
		/>
		<IconButton
			class="tapbtn"
			icon="download"

			label={m.drawer_export_label()}
			title={m.drawer_export_title()}
			onclick={onexport}
		/>
		<IconButton
			class="tapbtn fork-action"
			icon="fork"

			label={m.drawer_fork_label()}
			title={onforkselect ? m.drawer_fork_select_title() : m.drawer_fork_title()}
			aria-pressed={onforkselect ? forkSelectActive : undefined}
			onclick={onforkselect ?? onfork}
		/>
		</div>
		<!-- Mobile-only overflow toggle; hidden on desktop. -->
		<IconButton
			class="tapbtn more"
			icon="more"

			label={m.drawer_more_actions()}
			aria-expanded={moreOpen}
			title={m.drawer_more_actions()}
			onclick={() => (moreOpen = !moreOpen)}
		/>
		{#if !archived}
			<IconButton class="tapbtn interrupt" icon="stop"  label={m.drawer_interrupt_label()} title={m.drawer_interrupt_title()} onclick={oninterrupt} />
			<IconButton class="tapbtn archive" icon="archive"  label={m.drawer_archive()} onclick={onarchive} />
		{/if}
	</div>
	{#if session.labels.length > 0}
		<!-- Labels get their own full-width row in the header's column stack, so the
		     strip can spread edge-to-edge and wrap freely instead of being boxed
		     into the title row's leftover width (under the action buttons). The
		     empty-state trigger lives inline by the title (above), so this row only
		     appears once there's at least one label. -->
		<div class="hlabels">
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
		</div>
	{/if}
	<div class="hmeta row row-wrap">
		{#if showStatusBadge}<Badge class={statusBadgeClass(session.status)}>{session.status}</Badge>{/if}
		<WorkingDir path={session.working_dir} />
		<div class="meta-trail">
		<TokenUsage usage={session.token_usage} />
		<LangfuseChip id={session.id} />
		{#if isCodexSession && !archived}
			{#if modelEditing}
				<!-- display:contents wrapper exists only to give the compact Selects a
				     real scoped ancestor (.model-edit), so their width/fill reach-in
				     isn't a document-wide :global leak. -->
				<span class="model-edit">
					<Badge class="row" style="gap:var(--sp-1);padding:0.05rem var(--sp-1)">
						<Select compact chevron={false} bind:value={pendingModel} aria-label={m.drawer_model_aria()}>
							{#each codexModelOptions as opt (opt.v)}<option value={opt.v}>{opt.label}</option>{/each}
						</Select>
						<Select compact chevron={false} bind:value={pendingEffort} aria-label={m.drawer_effort_aria()}>
							{#each codexEffortOptions as e (e)}<option value={e}>{e || m.drawer_default_effort()}</option>{/each}
						</Select>
						<IconButton class="tapbtn" icon="check"  label={m.common_apply()} onclick={applyModelChange} />
						<IconButton class="tapbtn" icon="x"  label={m.common_cancel()} onclick={() => (modelEditing = false)} />
					</Badge>
				</span>
			{:else}
				<Badge
					as="button"
					mono
					title={m.drawer_change_model_title()}
					onclick={openModelEditor}
				>{session.model ?? m.drawer_default_model()}{session.effort ? ` · ${session.effort}` : ''} ✎</Badge>
			{/if}
		{:else if session.model || session.effort}
			<Badge
				as="button"
				mono
				title={m.drawer_no_inplace_model_title()}
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
		   viewport: a narrow-but-on-desktop drawer should fold its
		   buttons into the ⋯ flyout so the title stays visible. The header is the
		   size container the rules below query against. */
		container: drawer-head / inline-size;
	}
	.hrow {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		position: relative;
	}
	/* Labels on their own row so the strip spans the full header width. */
	.hlabels {
		display: flex;
		min-width: 0;
	}
	/* Secondary actions: inline on desktop, ⋯ flyout on mobile. */
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
		.dhead .secondary :global(.tapbtn) {
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
		.dhead .secondary :global(.tapbtn):hover {
			background: var(--bg-elevated-3, var(--bg-elevated-2));
		}
		/* Plain inline icon glyph inside a row — no chip box. */
		.dhead .secondary :global(.tapbtn svg) {
			flex: none;
		}
		.dhead .secondary :global(.tapbtn)::after {
			content: attr(aria-label);
			font-size: var(--fs-sm);
			font-weight: var(--fw-medium);
			/* Let a long label wrap inside the panel instead of clipping at the
			   viewport edge. */
			white-space: normal;
			text-align: left;
			line-height: 1.2;
		}
	}
	.dtitle {
		flex: 1;
		min-width: 0;
		display: flex;
		align-items: center;
		gap: var(--sp-1);
	}
	/* Title text shrink-wraps/ellipsises so the inline tag trigger sits right
	   after it; the dtitle box keeps flex:1 so the action buttons stay pinned
	   to the right edge. */
	.dtitle :global(.name) {
		flex: 0 1 auto;
		min-width: 0;
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
		color: var(--danger);
		border-color: color-mix(in srgb, var(--danger) 40%, var(--border-strong));
		background: color-mix(in srgb, var(--danger) 10%, var(--bg-elevated-2));
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
		color: var(--warn);
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
